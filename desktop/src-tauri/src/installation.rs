//! Native-only installation. No path, command, credential or binary is supplied by JavaScript.
use cafe_space_ipc::{
    BuildFlavor,
    client::Client,
    installation::{self as files, Handover, HandoverPhase},
    local,
    startup::{Action, Lease},
    updater::{self, acquire, blocking, marker_write, stopped_guard, system, terminal, wait_ready},
};
use cafe_space_protocol::{PrepareUpdateParams, Request, Response, UpdateMode, UpdateState};
use serde::Serialize;
use std::{
    io,
    path::PathBuf,
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
    pub restore_only: bool,
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
            restore_only: marker
                .as_ref()
                .is_some_and(|marker| marker.phase == HandoverPhase::RecoveryRequired),
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
        action: cafe_space_protocol::UpdateRecoveryAction,
        events: Channel<InstallationProgress>,
    ) -> Result<InstallationResult> {
        let _operation = self.operation.try_lock().map_err(|_| "INSTALL_BUSY")?;
        let finishing = action == cafe_space_protocol::UpdateRecoveryAction::Finish;
        progress(
            &events,
            if finishing {
                InstallationProgress::Starting
            } else {
                InstallationProgress::Recovering
            },
        );
        updater::recover(flavor, action).await?;
        progress(
            &events,
            if finishing {
                InstallationProgress::Committed
            } else {
                InstallationProgress::RolledBack
            },
        );
        Ok(if finishing {
            InstallationResult::Updated
        } else {
            InstallationResult::RolledBack
        })
    }
}

async fn rollback(
    flavor: BuildFlavor,
    lease: &Arc<Lease>,
    marker: Handover,
    events: &Channel<InstallationProgress>,
) -> Result<InstallationResult> {
    progress(events, InstallationProgress::Recovering);
    updater::rollback(flavor, lease, marker).await?;
    progress(events, InstallationProgress::RolledBack);
    Ok(InstallationResult::RolledBack)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::fs::File;
    use std::{
        path::Path,
        process::{Command, Stdio},
    };

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
                if std::thread::panicking()
                    && let Some(root) = BuildFlavor::Dev.data_dir()
                {
                    if let Ok(log) = std::fs::read_to_string(root.join("logs/daemon.log")) {
                        for line in log.lines().rev().take(24) {
                            eprintln!("M0_FAILURE_DAEMON {line}");
                        }
                    }
                    if let Ok(Some(marker)) = Handover::read(&root) {
                        eprintln!("M0_FAILURE_PHASE {:?}", marker.phase);
                    }
                }
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
            eprintln!("M0_FAULT_PROGRESS {body}");
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
                let file = held.lock().unwrap().take();
                assert!(
                    file.is_some(),
                    "fault handle must be released before rollback"
                );
                drop(file);
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
        let cli = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug")
            .join(if cfg!(windows) {
                "cafe-space-dev.exe"
            } else {
                "cafe-space-dev"
            });
        assert!(cli.is_file(), "build the headless CLI before the OS test");
        for (starting, headless, finish) in [
            (false, true, false),
            (false, true, true),
            (true, true, true),
            (true, false, true),
        ] {
            old_fixture(flavor, &source, &old_hash).await;
            interrupted_fixture(flavor, starting).await;
            if headless {
                let status = Command::new(&cli).arg("status").output().unwrap();
                assert!(status.status.success());
                assert!(
                    String::from_utf8(status.stdout)
                        .unwrap()
                        .contains("更新未完成")
                );
                assert!(
                    Command::new(&cli)
                        .args(["update", if finish { "finish" } else { "rollback" }])
                        .status()
                        .unwrap()
                        .success()
                );
            } else {
                assert!(matches!(
                    installer
                        .recover(
                            flavor,
                            cafe_space_protocol::UpdateRecoveryAction::Finish,
                            events()
                        )
                        .await
                        .unwrap(),
                    InstallationResult::Updated
                ));
            }
            assert_eq!(
                files::private_binary_hash(&program).unwrap(),
                if finish { HASH } else { &old_hash }
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
            "M0_HANDOVER_OS_OK install=true now=true whenIdle=true rollback=true recoveryCuts=true cliRecovery=true shellOnlyNoop=true statePreserved=true"
        );
    }
}
