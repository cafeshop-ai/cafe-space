//! Native-only installation. No path, command, credential or binary is supplied by JavaScript.
use cafe_space_ipc::{
    BuildFlavor,
    framing::{read_frame, write_frame},
    installation::{self as files, Handover, HandoverPhase},
    local,
    startup::{Action, Lease},
};
use cafe_space_protocol::{
    ClientHello, ClientMessage, ClientRequest, HealthResponse, HelloMessageType, PROTOCOL_VERSION,
    PrepareUpdateParams, Request, RequestMessageType, RequestResult, Response, ServerMessage,
    UpdateMode, UpdateState, decode_server_message, encode_client_message,
};
use serde::Serialize;
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::ipc::Channel;
use tokio::sync::{Mutex, watch};
use ts_rs::TS;

type Result<T> = std::result::Result<T, &'static str>;
const DEADLINE: Duration = Duration::from_secs(5);
const BINARY: &[u8] = include_bytes!(env!("CAFE_BUNDLED_DAEMON"));
const HASH: &str = env!("CAFE_BUNDLED_DAEMON_HASH");
const VERSION: &str = env!("CAFE_BUNDLED_DAEMON_VERSION");

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallationInfo {
    pub bundled_version: String,
    pub installed: bool,
    pub registered: bool,
    pub update_available: bool,
    pub recovery_required: bool,
}

#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InstallationProgress {
    Staging,
    Registering,
    Pending,
    Draining,
    Starting,
    Recovering,
    Committed,
    RolledBack,
}

#[derive(Clone, Copy, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InstallationResult {
    Installed,
    Updated,
    Unchanged,
    Cancelled,
    RolledBack,
}

pub struct Installer {
    operation: Mutex<()>,
    pending: AtomicBool,
    cancellation: watch::Sender<bool>,
}
impl Default for Installer {
    fn default() -> Self {
        Self {
            operation: Mutex::new(()),
            pending: AtomicBool::new(false),
            cancellation: watch::channel(false).0,
        }
    }
}

fn bundle(flavor: BuildFlavor) -> Result<()> {
    if env!("CAFE_BUNDLED_DAEMON_NAME") != flavor.daemon_name() {
        return Err("INSTALL_BUNDLE_FLAVOR_MISMATCH");
    }
    cafe_space_protocol::check_binary_hash(HASH).map_err(|_| "INSTALL_BUNDLE_INVALID")?;
    semver::Version::parse(VERSION).map_err(|_| "INSTALL_BUNDLE_INVALID")?;
    Ok(())
}
fn check_upgrade(current: &str, target: &str) -> Result<()> {
    let current = semver::Version::parse(current).map_err(|_| "INSTALL_VERSION_INVALID")?;
    let target = semver::Version::parse(target).map_err(|_| "INSTALL_BUNDLE_INVALID")?;
    if current.cmp_precedence(&target).is_gt() {
        return Err("INSTALL_DOWNGRADE_REFUSED");
    }
    Ok(())
}
fn progress(events: &Channel<InstallationProgress>, phase: InstallationProgress) {
    let _ = events.send(phase);
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
    code: &'static str,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| "INSTALL_WORKER_FAILED")?
        .map_err(|_| code)
}
async fn acquire(flavor: BuildFlavor) -> Result<Arc<Lease>> {
    blocking(
        move || Lease::acquire(flavor).map(Arc::new),
        "INSTALL_LEASE_UNAVAILABLE",
    )
    .await
}
async fn system(lease: &Arc<Lease>, action: Action) -> Result<()> {
    let lease = lease.clone();
    blocking(move || lease.execute(action), "INSTALL_SERVICE_FAILED").await
}
async fn marker_write(lease: &Arc<Lease>, marker: Handover) -> Result<()> {
    let lease = lease.clone();
    blocking(move || marker.write(lease.root()), "INSTALL_MARKER_FAILED").await
}
fn terminal(phase: HandoverPhase) -> bool {
    matches!(
        phase,
        HandoverPhase::Committed | HandoverPhase::RolledBack | HandoverPhase::NotApplied
    )
}
fn installation_paths(flavor: BuildFlavor) -> Result<(PathBuf, PathBuf)> {
    Ok((
        flavor.data_dir().ok_or("INSTALL_PATH_UNAVAILABLE")?,
        flavor
            .installed_daemon()
            .ok_or("INSTALL_PATH_UNAVAILABLE")?,
    ))
}

impl Installer {
    pub async fn info(&self, flavor: BuildFlavor) -> Result<InstallationInfo> {
        bundle(flavor)?;
        let (root, program) = installation_paths(flavor)?;
        let digest = blocking(
            move || match std::fs::symlink_metadata(&program) {
                Ok(_) => files::private_binary_hash(&program).map(Some),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            },
            "INSTALL_BINARY_INVALID",
        )
        .await?;
        let registered = if digest.is_some() {
            blocking(
                move || cafe_space_ipc::startup::verify(flavor),
                "INSTALL_SERVICE_UNAVAILABLE",
            )
            .await
            .is_ok()
        } else {
            false
        };
        let marker = blocking(move || Handover::read(&root), "INSTALL_MARKER_INVALID").await?;
        Ok(InstallationInfo {
            bundled_version: VERSION.into(),
            installed: digest.is_some(),
            registered,
            update_available: digest.as_deref().is_some_and(|hash| hash != HASH),
            recovery_required: marker.is_some_and(|marker| !terminal(marker.phase)),
        })
    }
    pub fn cancel(&self) -> Result<()> {
        if !self.pending.load(Ordering::Acquire) {
            return Err("UPDATE_NOT_PENDING");
        }
        self.cancellation.send_replace(true);
        Ok(())
    }
    pub async fn install(
        &self,
        flavor: BuildFlavor,
        events: Channel<InstallationProgress>,
    ) -> Result<InstallationResult> {
        let _operation = self.operation.try_lock().map_err(|_| "INSTALL_BUSY")?;
        bundle(flavor)?;
        let lease = acquire(flavor).await?;
        if Handover::read(lease.root())
            .map_err(|_| "INSTALL_MARKER_INVALID")?
            .is_some_and(|marker| !terminal(marker.phase))
        {
            return Err("INSTALL_RECOVERY_REQUIRED");
        }
        let endpoint = local::endpoint(flavor).map_err(|_| "INSTALL_PATH_UNAVAILABLE")?;
        if tokio::time::timeout(DEADLINE, local::connect(&endpoint))
            .await
            .map_err(|_| "INSTALL_CONNECT_TIMEOUT")?
            .is_ok()
        {
            return Err("INSTALL_DAEMON_ALREADY_RUNNING");
        }
        progress(&events, InstallationProgress::Staging);
        let copy = lease.clone();
        blocking(
            move || {
                match files::private_binary_hash(copy.program()) {
                    Ok(hash) if hash == HASH => return Ok(()),
                    Ok(_) => return Err(io::Error::other("existing daemon requires update")),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                files::stage_bytes(BINARY, &copy.program().with_extension("next"), HASH)?;
                files::install_staged(copy.program(), HASH)
            },
            "INSTALL_COPY_FAILED",
        )
        .await?;
        progress(&events, InstallationProgress::Registering);
        if system(&lease, Action::Verify).await.is_err() {
            system(&lease, Action::Install).await?;
        }
        progress(&events, InstallationProgress::Starting);
        // A timeout is not retried: only a matching live handshake can resolve an unknown start outcome.
        let _start = system(&lease, Action::Start).await;
        let health = wait_ready(flavor, lease.root(), HASH).await?;
        if health.daemon_version != VERSION {
            return Err("INSTALL_VERSION_MISMATCH");
        }
        progress(&events, InstallationProgress::Committed);
        Ok(InstallationResult::Installed)
    }
    pub async fn start(&self, flavor: BuildFlavor) -> Result<InstallationResult> {
        let _operation = self.operation.try_lock().map_err(|_| "INSTALL_BUSY")?;
        let lease = acquire(flavor).await?;
        if Handover::read(lease.root())
            .map_err(|_| "INSTALL_MARKER_INVALID")?
            .is_some_and(|marker| !terminal(marker.phase))
        {
            return Err("INSTALL_RECOVERY_REQUIRED");
        }
        let hash =
            files::private_binary_hash(lease.program()).map_err(|_| "INSTALL_BINARY_INVALID")?;
        system(&lease, Action::Verify).await?;
        let _start = system(&lease, Action::Start).await;
        wait_ready(flavor, lease.root(), &hash).await?;
        Ok(InstallationResult::Unchanged)
    }
    pub async fn update(
        &self,
        flavor: BuildFlavor,
        mode: UpdateMode,
        events: Channel<InstallationProgress>,
    ) -> Result<InstallationResult> {
        let _operation = self.operation.try_lock().map_err(|_| "INSTALL_BUSY")?;
        bundle(flavor)?;
        self.pending.store(false, Ordering::Release);
        self.cancellation.send_replace(false);
        let result = self.update_inner(flavor, mode, &events).await;
        self.pending.store(false, Ordering::Release);
        result
    }
    async fn update_inner(
        &self,
        flavor: BuildFlavor,
        mode: UpdateMode,
        events: &Channel<InstallationProgress>,
    ) -> Result<InstallationResult> {
        let lease = acquire(flavor).await?;
        if Handover::read(lease.root())
            .map_err(|_| "INSTALL_MARKER_INVALID")?
            .is_some_and(|marker| !terminal(marker.phase))
        {
            return Err("INSTALL_RECOVERY_REQUIRED");
        }
        let (mut client, health) = Client::connect(flavor, lease.root()).await?;
        let old = health.binary_hash.ok_or("INSTALL_DAEMON_UNSUPPORTED")?;
        if old == HASH {
            return Ok(InstallationResult::Unchanged);
        }
        check_upgrade(&health.daemon_version, VERSION)?;
        if files::private_binary_hash(lease.program()).map_err(|_| "INSTALL_BINARY_INVALID")? != old
        {
            return Err("INSTALL_RUNNING_BINARY_MISMATCH");
        }
        system(&lease, Action::Verify).await?;
        progress(events, InstallationProgress::Staging);
        let copy = lease.clone();
        blocking(
            move || files::stage_bytes(BINARY, &copy.program().with_extension("next"), HASH),
            "INSTALL_STAGE_FAILED",
        )
        .await?;
        let prepared = client
            .request(Request::PrepareUpdate(PrepareUpdateParams {
                mode,
                target_hash: HASH.into(),
            }))
            .await;
        let mut state = match prepared {
            Ok(Response::Update(update)) => update.update,
            _ => UpdateState::Draining,
        };
        let mut cancelled = self.cancellation.subscribe();
        if state == UpdateState::Pending {
            self.pending.store(true, Ordering::Release);
            progress(events, InstallationProgress::Pending);
        }
        while state == UpdateState::Pending {
            tokio::select! { _ = cancelled.changed() => {}, _ = tokio::time::sleep(Duration::from_millis(250)) => {} }
            let cancel = *cancelled.borrow();
            if cancel {
                self.pending.store(false, Ordering::Release);
                if matches!(client.request(Request::CancelUpdate).await, Ok(Response::Update(update)) if update.update == UpdateState::None)
                {
                    return Ok(InstallationResult::Cancelled);
                }
                break;
            }
            state = match client.request(Request::GetStatus).await {
                Ok(Response::Health(health)) => health.update,
                _ => break,
            };
        }
        self.pending.store(false, Ordering::Release);
        if state == UpdateState::None {
            return Ok(InstallationResult::Cancelled);
        }
        let mut marker = Handover::read(lease.root())
            .map_err(|_| "INSTALL_MARKER_INVALID")?
            .ok_or("INSTALL_PREPARE_UNCONFIRMED")?;
        if marker.old_hash != old
            || marker.target_hash != HASH
            || marker.phase != HandoverPhase::Draining
        {
            return Err("INSTALL_PREPARE_UNCONFIRMED");
        }
        drop(client);
        progress(events, InstallationProgress::Draining);
        let stopped = stopped_guard(flavor, lease.root()).await?;
        system(&lease, Action::Stop).await?;
        marker.phase = HandoverPhase::Swapping;
        marker_write(&lease, marker.clone()).await?;
        let swap = lease.clone();
        let old_hash = old.clone();
        let swapped = blocking(
            move || files::activate_staged(swap.program(), &old_hash, HASH),
            "INSTALL_SWAP_FAILED",
        )
        .await;
        drop(stopped);
        if swapped.is_err() {
            return rollback(flavor, &lease, marker, events).await;
        }
        marker.phase = HandoverPhase::Starting;
        marker_write(&lease, marker.clone()).await?;
        progress(events, InstallationProgress::Starting);
        let _start = system(&lease, Action::Start).await;
        match wait_ready(flavor, lease.root(), HASH).await {
            Ok(health) if health.daemon_version == VERSION => {
                marker.phase = HandoverPhase::Committed;
                marker_write(&lease, marker).await?;
                // The follow-up read releases the target's provisional admission gate.
                let (_, health) = Client::connect(flavor, lease.root()).await?;
                if health.update != UpdateState::None {
                    return Err("INSTALL_COMMIT_UNCONFIRMED");
                }
                progress(events, InstallationProgress::Committed);
                Ok(InstallationResult::Updated)
            }
            _ => rollback(flavor, &lease, marker, events).await,
        }
    }
    pub async fn recover(
        &self,
        flavor: BuildFlavor,
        events: Channel<InstallationProgress>,
    ) -> Result<InstallationResult> {
        let _operation = self.operation.try_lock().map_err(|_| "INSTALL_BUSY")?;
        let lease = acquire(flavor).await?;
        let marker = Handover::read(lease.root())
            .map_err(|_| "INSTALL_MARKER_INVALID")?
            .ok_or("INSTALL_NO_RECOVERY")?;
        if terminal(marker.phase) {
            return Ok(InstallationResult::Unchanged);
        }
        // Recovery prefers a confirmed target; otherwise the old binary must prove state compatibility.
        if let Ok((_, health)) = Client::connect(flavor, lease.root()).await
            && health.binary_hash.as_deref() == Some(&marker.target_hash)
            && files::private_binary_hash(lease.program()).map_err(|_| "INSTALL_BINARY_INVALID")?
                == marker.target_hash
        {
            let mut committed = marker;
            committed.phase = HandoverPhase::Committed;
            marker_write(&lease, committed).await?;
            let (_, health) = Client::connect(flavor, lease.root()).await?;
            if health.update != UpdateState::None {
                return Err("INSTALL_COMMIT_UNCONFIRMED");
            }
            progress(&events, InstallationProgress::Committed);
            return Ok(InstallationResult::Updated);
        }
        rollback(flavor, &lease, marker, &events).await
    }
}

async fn stopped_guard(flavor: BuildFlavor, root: &Path) -> Result<File> {
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let path = root.join(format!("{}.lock", flavor.daemon_name()));
        let lock = blocking(
            move || {
                let file = cafe_space_ipc::private::existing_file(&path)?;
                match file.try_lock() {
                    Ok(()) => Ok(Some(file)),
                    Err(std::fs::TryLockError::WouldBlock) => Ok(None),
                    Err(std::fs::TryLockError::Error(error)) => Err(error),
                }
            },
            "INSTALL_STOP_UNCONFIRMED",
        )
        .await?;
        if let Some(lock) = lock {
            return Ok(lock);
        }
        if tokio::time::Instant::now() >= until {
            return Err("INSTALL_STOP_UNCONFIRMED");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn wait_ready(flavor: BuildFlavor, root: &Path, expected: &str) -> Result<HealthResponse> {
    let until = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok((_, health)) = Client::connect(flavor, root).await
            && health.ready
            && health.binary_hash.as_deref() == Some(expected)
        {
            return Ok(health);
        }
        if tokio::time::Instant::now() >= until {
            return Err("INSTALL_START_UNCONFIRMED");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn rollback(
    flavor: BuildFlavor,
    lease: &Arc<Lease>,
    mut marker: Handover,
    events: &Channel<InstallationProgress>,
) -> Result<InstallationResult> {
    progress(events, InstallationProgress::Recovering);
    let restored = async {
        system(lease, Action::Verify).await?;
        system(lease, Action::Stop).await?;
        drop(stopped_guard(flavor, lease.root()).await?);
        let program = match files::private_binary_hash(lease.program()) {
            Ok(hash) if hash == marker.old_hash => lease.program().to_owned(),
            _ => lease.program().with_extension("prev"),
        };
        let old = marker.old_hash.clone();
        blocking(
            move || {
                if files::private_binary_hash(&program)? != old {
                    return Err(io::Error::other("old binary hash mismatch"));
                }
                let mut child = Command::new(program)
                    .arg("--check-state")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?;
                let until = std::time::Instant::now() + Duration::from_secs(15);
                loop {
                    if let Some(status) = child.try_wait()? {
                        return if status.success() {
                            Ok(())
                        } else {
                            Err(io::Error::other("old binary refused current state"))
                        };
                    }
                    if std::time::Instant::now() >= until {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(io::Error::other("old state check timed out"));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            },
            "INSTALL_ROLLBACK_STATE_REFUSED",
        )
        .await?;
        let stopped = stopped_guard(flavor, lease.root()).await?;
        let copy = lease.clone();
        let old = marker.old_hash.clone();
        let target = marker.target_hash.clone();
        blocking(
            move || files::restore_previous(copy.program(), &old, &target),
            "INSTALL_ROLLBACK_BINARY_FAILED",
        )
        .await?;
        marker.phase = HandoverPhase::RolledBack;
        marker_write(lease, marker.clone()).await?;
        drop(stopped);
        let _start = system(lease, Action::Start).await;
        wait_ready(flavor, lease.root(), &marker.old_hash).await?;
        Ok::<(), &'static str>(())
    }
    .await;
    if let Err(error) = restored {
        marker.phase = HandoverPhase::RecoveryRequired;
        marker_write(lease, marker).await?;
        return Err(error);
    }
    progress(events, InstallationProgress::RolledBack);
    Ok(InstallationResult::RolledBack)
}

struct Client {
    stream: local::ClientStream,
    next: u64,
}
impl Client {
    async fn connect(flavor: BuildFlavor, root: &Path) -> Result<(Self, HealthResponse)> {
        let endpoint = local::endpoint(flavor).map_err(|_| "INSTALL_PATH_UNAVAILABLE")?;
        Self::connect_at(&endpoint, root).await
    }
    async fn connect_at(endpoint: &Path, root: &Path) -> Result<(Self, HealthResponse)> {
        let identity = files::daemon_identity(root).map_err(|_| "INSTALL_IDENTITY_UNAVAILABLE")?;
        let mut stream = tokio::time::timeout(DEADLINE, local::connect(endpoint))
            .await
            .map_err(|_| "INSTALL_CONNECT_TIMEOUT")?
            .map_err(|_| "INSTALL_CONNECT_FAILED")?;
        #[cfg(windows)]
        if local::server_process_id(&stream).map_err(|_| "INSTALL_IDENTITY_UNAVAILABLE")?
            != identity.pid
        {
            return Err("INSTALL_PEER_MISMATCH");
        }
        let hello = ClientMessage::Hello(ClientHello {
            message_type: HelloMessageType::Hello,
            supported_protocol_versions: vec![PROTOCOL_VERSION],
            client_version: env!("CARGO_PKG_VERSION").into(),
        });
        let bytes = encode_client_message(&hello).map_err(|_| "INSTALL_CODEC_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut stream, &bytes))
            .await
            .map_err(|_| "INSTALL_HELLO_TIMEOUT")?
            .map_err(|_| "INSTALL_HELLO_FAILED")?;
        let mut client = Self { stream, next: 0 };
        if !matches!(client.read().await?, ServerMessage::Welcome(_)) {
            return Err("INSTALL_PROTOCOL_MISMATCH");
        }
        let Response::Health(health) = client.request(Request::GetStatus).await? else {
            return Err("INSTALL_PROTOCOL_MISMATCH");
        };
        if health.event_stream_id.as_ref() != Some(&identity.event_stream_id)
            || health.binary_hash.is_none()
        {
            return Err("INSTALL_PEER_MISMATCH");
        }
        Ok((client, health))
    }
    async fn read(&mut self) -> Result<ServerMessage> {
        let bytes = tokio::time::timeout(DEADLINE, read_frame(&mut self.stream))
            .await
            .map_err(|_| "INSTALL_READ_TIMEOUT")?
            .map_err(|_| "INSTALL_READ_FAILED")?
            .ok_or("INSTALL_CONNECTION_CLOSED")?;
        decode_server_message(&bytes).map_err(|_| "INSTALL_CODEC_FAILED")
    }
    async fn request(&mut self, request: Request) -> Result<Response> {
        self.next = self.next.checked_add(1).ok_or("INSTALL_REQUEST_LIMIT")?;
        let id = format!("native-install-{}", self.next);
        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: id.clone(),
            request,
        });
        let bytes = encode_client_message(&request).map_err(|_| "INSTALL_CODEC_FAILED")?;
        tokio::time::timeout(DEADLINE, write_frame(&mut self.stream, &bytes))
            .await
            .map_err(|_| "INSTALL_WRITE_TIMEOUT")?
            .map_err(|_| "INSTALL_WRITE_FAILED")?;
        match self.read().await? {
            ServerMessage::Response(response) if response.request_id == id => match response.result
            {
                RequestResult::Applied(data) => Ok(data.result),
                _ => Err("INSTALL_REQUEST_REJECTED"),
            },
            _ => Err("INSTALL_PROTOCOL_MISMATCH"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn refuses_downgrades_but_not_build_metadata_changes() {
        assert!(check_upgrade("0.2.0", "0.1.0").is_err());
        assert!(check_upgrade("0.1.0", "0.1.0-rc.1").is_err());
        assert!(check_upgrade("invalid", "0.1.0").is_err());
        assert!(check_upgrade("0.1.0+build.9", "0.1.0+build.1").is_ok());
        assert!(check_upgrade("0.1.0-rc.1", "0.1.0").is_ok());
    }

    #[test]
    fn embedded_artifact_is_staged_with_its_compiled_digest() {
        bundle(crate::build_flavor()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("private/daemon.next");
        files::stage_bytes(BINARY, &target, HASH).unwrap();
        assert_eq!(files::private_binary_hash(&target).unwrap(), HASH);
        assert!(
            bundle(if cfg!(debug_assertions) {
                BuildFlavor::Release
            } else {
                BuildFlavor::Dev
            })
            .is_err()
        );
        assert!(Installer::default().cancel().is_err());
    }

    fn db_probe(root: &Path, create: bool) {
        let script = if create {
            "import os,sqlite3; c=sqlite3.connect(os.environ['CAFE_TEST_DB']); c.execute('CREATE TABLE handover_probe(value TEXT NOT NULL)'); c.execute(\"INSERT INTO handover_probe VALUES ('m0-handover-kept')\"); c.commit(); c.close()"
        } else {
            "import os,sqlite3,pathlib; c=sqlite3.connect(pathlib.Path(os.environ['CAFE_TEST_DB']).resolve().as_uri()+'?mode=ro',uri=True); assert c.execute('SELECT value FROM handover_probe').fetchall()==[('m0-handover-kept',)]; c.close()"
        };
        assert!(
            Command::new(if cfg!(windows) { "python" } else { "python3" })
                .args(["-c", script])
                .env("CAFE_TEST_DB", root.join("state.sqlite3"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("hosted runner Python with SQLite")
                .success(),
            "database sentinel must survive binary handover"
        );
    }

    async fn old_fixture(flavor: BuildFlavor, source: &Path, old_hash: &str) {
        let lease = acquire(flavor).await.unwrap();
        system(&lease, Action::Stop).await.unwrap();
        let stopped = stopped_guard(flavor, lease.root()).await.unwrap();
        let current = files::private_binary_hash(lease.program()).unwrap();
        if current != old_hash {
            files::stage_binary(source, &lease.program().with_extension("next"), old_hash).unwrap();
            files::activate_staged(lease.program(), &current, old_hash).unwrap();
        }
        drop(stopped);
        system(&lease, Action::Start).await.unwrap();
        wait_ready(flavor, lease.root(), old_hash).await.unwrap();
    }

    async fn interrupted_fixture(flavor: BuildFlavor, start_target: bool) {
        let lease = acquire(flavor).await.unwrap();
        let (mut client, _) = Client::connect(flavor, lease.root()).await.unwrap();
        files::stage_bytes(BINARY, &lease.program().with_extension("next"), HASH).unwrap();
        assert!(
            matches!(client.request(Request::PrepareUpdate(PrepareUpdateParams { mode: UpdateMode::Now, target_hash: HASH.into() })).await.unwrap(), Response::Update(update) if update.update == UpdateState::Draining)
        );
        drop(client);
        let stopped = stopped_guard(flavor, lease.root()).await.unwrap();
        system(&lease, Action::Stop).await.unwrap();
        let mut marker = Handover::read(lease.root()).unwrap().unwrap();
        marker.phase = HandoverPhase::Swapping;
        marker.write(lease.root()).unwrap();
        if start_target {
            files::activate_staged(lease.program(), &marker.old_hash, HASH).unwrap();
            marker.phase = HandoverPhase::Starting;
            marker.write(lease.root()).unwrap();
            drop(stopped);
            system(&lease, Action::Start).await.unwrap();
            wait_ready(flavor, lease.root(), HASH).await.unwrap();
        } else {
            cafe_space_ipc::private::replace_durable(
                lease.program(),
                &lease.program().with_extension("prev"),
            )
            .unwrap();
            drop(stopped);
        }
        // Dropping only the process-owned handles leaves exactly the durable cut state for recovery.
    }

    #[tokio::test]
    #[ignore = "changes user login registration; disposable GitHub-hosted runners only"]
    async fn os_manager_install_update_and_rollback() {
        assert_eq!(
            std::env::var("CAFE_SPACE_TEST_HANDOVER").as_deref(),
            Ok("1")
        );
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(
            std::env::var("RUNNER_ENVIRONMENT").as_deref(),
            Ok("github-hosted")
        );
        assert!(cfg!(debug_assertions));
        let flavor = BuildFlavor::Dev;
        let (root, program) = installation_paths(flavor).unwrap();
        assert!(
            !root.exists(),
            "refuse any existing development installation or database"
        );
        let endpoint = local::endpoint(flavor).unwrap();
        assert!(
            local::connect(&endpoint).await.is_err(),
            "never adopt an existing dev daemon"
        );
        let source = PathBuf::from(
            std::env::var_os("CAFE_SPACE_M0_OLD_BINARY").expect("isolated old artifact"),
        );
        let old_hash = files::binary_hash(&source).unwrap();
        assert_ne!(
            old_hash, HASH,
            "old and new builds must have different binary hashes"
        );
        struct Cleanup;
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if cafe_space_ipc::startup::verify(BuildFlavor::Dev).is_ok() {
                    let _ = cafe_space_ipc::startup::execute(BuildFlavor::Dev, Action::Uninstall);
                }
            }
        }
        let cleanup = Cleanup;
        let installer = Installer::default();
        let events = || Channel::new(|_| Ok(()));
        assert!(matches!(
            installer.install(flavor, events()).await.unwrap(),
            InstallationResult::Installed
        ));
        assert!(installer.info(flavor).await.unwrap().registered);
        db_probe(&root, true);
        let unchanged = files::daemon_identity(&root).unwrap();
        assert!(matches!(
            installer
                .update(flavor, UpdateMode::Now, events())
                .await
                .unwrap(),
            InstallationResult::Unchanged
        ));
        assert_eq!(
            files::daemon_identity(&root).unwrap(),
            unchanged,
            "shell-only updates must not restart the daemon"
        );
        for mode in [UpdateMode::Now, UpdateMode::WhenIdle] {
            old_fixture(flavor, &source, &old_hash).await;
            let before = files::daemon_identity(&root).unwrap();
            assert!(matches!(
                installer.update(flavor, mode, events()).await.unwrap(),
                InstallationResult::Updated
            ));
            assert_ne!(
                files::daemon_identity(&root).unwrap().event_stream_id,
                before.event_stream_id
            );
            assert_eq!(files::private_binary_hash(&program).unwrap(), HASH);
            assert_eq!(
                files::private_binary_hash(&program.with_extension("prev")).unwrap(),
                old_hash
            );
            assert_eq!(
                Handover::read(&root).unwrap().unwrap().phase,
                HandoverPhase::Committed
            );
            db_probe(&root, false);
        }
        old_fixture(flavor, &source, &old_hash).await;
        let injected = Arc::new(AtomicBool::new(false));
        let injection = injected.clone();
        #[cfg(windows)]
        let blocker = Arc::new(std::sync::Mutex::new(None::<File>));
        #[cfg(windows)]
        let held = blocker.clone();
        let target = program.clone();
        // Fault injection exists only in this ignored test, never in normal/debug/release app commands.
        let failures = Channel::new(move |body| {
            let tauri::ipc::InvokeResponseBody::Json(body) = body else {
                panic!("JSON progress required")
            };
            if body == "\"starting\"" && !injection.swap(true, Ordering::SeqCst) {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    *held.lock().unwrap() = Some(
                        std::fs::OpenOptions::new()
                            .read(true)
                            .share_mode(0)
                            .open(&target)
                            .unwrap(),
                    );
                }
            }
            #[cfg(windows)]
            if body == "\"recovering\"" {
                drop(held.lock().unwrap().take());
            }
            Ok(())
        });
        assert!(matches!(
            installer
                .update(flavor, UpdateMode::Now, failures)
                .await
                .unwrap(),
            InstallationResult::RolledBack
        ));
        assert!(injected.load(Ordering::SeqCst));
        assert_eq!(files::private_binary_hash(&program).unwrap(), old_hash);
        assert_eq!(
            Handover::read(&root).unwrap().unwrap().phase,
            HandoverPhase::RolledBack
        );
        db_probe(&root, false);
        for starting in [false, true] {
            old_fixture(flavor, &source, &old_hash).await;
            interrupted_fixture(flavor, starting).await;
            let result = installer.recover(flavor, events()).await.unwrap();
            assert!(if starting {
                matches!(result, InstallationResult::Updated)
            } else {
                matches!(result, InstallationResult::RolledBack)
            });
            assert_eq!(
                files::private_binary_hash(&program).unwrap(),
                if starting { HASH } else { &old_hash }
            );
            assert!(terminal(Handover::read(&root).unwrap().unwrap().phase));
            db_probe(&root, false);
        }
        cafe_space_ipc::startup::execute(flavor, Action::Uninstall).unwrap();
        assert!(
            root.join("state.sqlite3").exists(),
            "uninstall retains database"
        );
        db_probe(&root, false);
        drop(cleanup);
        println!(
            "M0_HANDOVER_OS_OK install=true now=true whenIdle=true rollback=true recoveryCuts=true shellOnlyNoop=true statePreserved=true"
        );
    }

    #[tokio::test]
    async fn client_refuses_another_boot_before_any_mutation() {
        for matching in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            let _guard = cafe_space_ipc::private::directory(&root).unwrap();
            let mut record = cafe_space_ipc::private::file(&root.join("daemon.instance")).unwrap();
            writeln!(record, "{} {}", std::process::id(), "a".repeat(32)).unwrap();
            drop(record);
            #[cfg(unix)]
            let endpoint = temp.path().join("s");
            #[cfg(windows)]
            let endpoint = PathBuf::from(format!(
                r"\\.\pipe\cafe-native-install-{}-{}",
                std::process::id(),
                temp.path().file_name().unwrap().to_string_lossy()
            ));
            let mut listener = local::Listener::bind(&endpoint).await.unwrap();
            let serving = tokio::spawn(async move {
                let mut stream = listener.accept().await.unwrap();
                let _hello = read_frame(&mut stream).await.unwrap().unwrap();
                write_frame(
                    &mut stream,
                    br#"{"type":"welcome","protocolVersion":2,"daemonVersion":"0.1.0"}"#,
                )
                .await
                .unwrap();
                let request = cafe_space_protocol::decode_client_message(
                    &read_frame(&mut stream).await.unwrap().unwrap(),
                )
                .unwrap();
                let ClientMessage::Request(request) = request else {
                    panic!("expected health request")
                };
                assert_eq!(request.request, Request::GetStatus);
                let id = if matching { "a" } else { "b" }.repeat(32);
                let response = format!(
                    r#"{{"type":"response","protocolVersion":2,"requestId":"{}","result":{{"status":"applied","data":{{"result":{{"kind":"health","data":{{"daemonVersion":"0.1.0","ready":true,"binaryHash":"{}","eventStreamId":"{id}"}}}}}}}}}}"#,
                    request.request_id,
                    "c".repeat(64)
                );
                write_frame(&mut stream, response.as_bytes()).await.unwrap();
                assert!(
                    read_frame(&mut stream).await.unwrap().is_none(),
                    "no prepare_update may be sent before identity validation"
                );
            });
            let result = Client::connect_at(&endpoint, &root).await;
            assert_eq!(result.is_ok(), matching);
            drop(result);
            tokio::time::timeout(DEADLINE, serving)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
