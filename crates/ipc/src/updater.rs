//! System-managed handover recovery shared by the desktop and headless CLI.
use crate::{
    BuildFlavor,
    client::Client,
    installation::{self as files, Handover, HandoverPhase},
    startup::{Action, Lease},
};
use cafe_space_protocol::HealthResponse;
use std::{
    fs::File,
    io,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
pub type Result<T> = std::result::Result<T, &'static str>;
pub use cafe_space_protocol::{UpdateRecoveryAction, UpdateState};

pub async fn request_shutdown(flavor: BuildFlavor, root: &Path) -> Result<()> {
    let (mut client, _) = Client::connect(flavor, root).await?;
    let reply = client.request(cafe_space_protocol::Request::Shutdown).await;
    drop(client);
    drop(stopped_guard(flavor, root).await?);
    if matches!(reply, Ok(cafe_space_protocol::Response::ShutdownAccepted)) {
        Ok(())
    } else {
        Err("INSTALL_STOP_ACK_UNCONFIRMED")
    }
}

pub async fn recover(flavor: BuildFlavor, action: UpdateRecoveryAction) -> Result<()> {
    let lease = acquire(flavor).await?;
    let marker = Handover::read(lease.root())
        .map_err(|_| "INSTALL_MARKER_INVALID")?
        .ok_or("INSTALL_NO_RECOVERY")?;
    if terminal(marker.phase) {
        return Err("INSTALL_NO_RECOVERY");
    }
    match action {
        UpdateRecoveryAction::Finish => finish(flavor, &lease, marker).await,
        UpdateRecoveryAction::Rollback => rollback(flavor, &lease, marker).await,
    }
}

async fn finish(flavor: BuildFlavor, lease: &Arc<Lease>, mut marker: Handover) -> Result<()> {
    if marker.phase == HandoverPhase::RecoveryRequired {
        return Err("INSTALL_EXPLICIT_RESTORE_REQUIRED");
    }
    let installed = files::private_binary_hash(lease.program());
    if matches!(&installed, Ok(hash) if hash == &marker.target_hash)
        && let Ok((_, health)) = Client::connect(flavor, lease.root()).await
        && health.binary_hash.as_deref() == Some(&marker.target_hash)
        && (health.ready || health.awaiting_update_confirmation == Some(true))
    {
        return commit(flavor, lease, marker).await;
    }
    // Never stop an old process that has resumed work: an active drain must first exit by itself.
    let stopped = stopped_guard(flavor, lease.root()).await?;
    system(lease, Action::Verify).await?;
    system(lease, Action::Stop).await?;
    if Handover::read(lease.root())
        .map_err(|_| "INSTALL_MARKER_INVALID")?
        .as_ref()
        != Some(&marker)
    {
        return Err("INSTALL_HANDOVER_CHANGED");
    }
    let target = marker.target_hash.clone();
    let installed = files::private_binary_hash(lease.program());
    match installed {
        Ok(hash) if hash == target => {}
        Ok(hash) if hash == marker.old_hash => {
            marker.phase = HandoverPhase::Swapping;
            marker_write(lease, marker.clone()).await?;
            let lease = lease.clone();
            let old = marker.old_hash.clone();
            blocking(
                move || files::activate_staged(lease.program(), &old, &target),
                "INSTALL_SWAP_FAILED",
            )
            .await?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if files::private_binary_hash(&lease.program().with_extension("prev"))
                .map_err(|_| "INSTALL_BINARY_INVALID")?
                != marker.old_hash
            {
                return Err("INSTALL_BINARY_INVALID");
            }
            let lease = lease.clone();
            blocking(
                move || files::install_staged(lease.program(), &target),
                "INSTALL_SWAP_FAILED",
            )
            .await?;
        }
        _ => return Err("INSTALL_BINARY_INVALID"),
    }
    marker.phase = HandoverPhase::Starting;
    marker_write(lease, marker.clone()).await?;
    drop(stopped);
    let _start = system(lease, Action::Start).await;
    wait_ready(flavor, lease.root(), &marker.target_hash).await?;
    commit(flavor, lease, marker).await
}

async fn commit(flavor: BuildFlavor, lease: &Arc<Lease>, mut marker: Handover) -> Result<()> {
    let expected = marker.target_hash.clone();
    marker.phase = HandoverPhase::Committed;
    marker_write(lease, marker).await?;
    let (_, health) = Client::connect(flavor, lease.root()).await?;
    if !health.ready
        || health.awaiting_update_confirmation == Some(true)
        || health.binary_hash.as_deref() != Some(&expected)
    {
        return Err("INSTALL_COMMIT_UNCONFIRMED");
    }
    Ok(())
}

pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
    code: &'static str,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| "INSTALL_WORKER_FAILED")?
        .map_err(|_| code)
}
pub async fn acquire(flavor: BuildFlavor) -> Result<Arc<Lease>> {
    blocking(
        move || Lease::acquire(flavor).map(Arc::new),
        "INSTALL_LEASE_UNAVAILABLE",
    )
    .await
}
pub async fn system(lease: &Arc<Lease>, action: Action) -> Result<()> {
    let lease = lease.clone();
    blocking(
        move || {
            lease.execute(action).inspect_err(|error| {
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr(),
                    "INSTALL_SERVICE_FAILED action={action:?} kind={:?}",
                    error.kind()
                );
            })
        },
        "INSTALL_SERVICE_FAILED",
    )
    .await
}
pub async fn marker_write(lease: &Arc<Lease>, marker: Handover) -> Result<()> {
    let lease = lease.clone();
    blocking(move || marker.write(lease.root()), "INSTALL_MARKER_FAILED").await
}
pub fn terminal(phase: HandoverPhase) -> bool {
    matches!(
        phase,
        HandoverPhase::Committed | HandoverPhase::RolledBack | HandoverPhase::NotApplied
    )
}
pub async fn stopped_guard(flavor: BuildFlavor, root: &Path) -> Result<File> {
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let path = root.join(format!("{}.lock", flavor.daemon_name()));
        let lock = blocking(
            move || {
                let file = crate::private::existing_file(&path)?;
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
pub async fn wait_ready(
    flavor: BuildFlavor,
    root: &Path,
    expected: &str,
) -> Result<HealthResponse> {
    let until = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok((_, health)) = Client::connect(flavor, root).await
            && (health.ready || health.awaiting_update_confirmation == Some(true))
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
pub async fn rollback(flavor: BuildFlavor, lease: &Arc<Lease>, mut marker: Handover) -> Result<()> {
    if let Ok((_, health)) = Client::connect(flavor, lease.root()).await
        && health.binary_hash.as_deref() == Some(&marker.old_hash)
        && health.ready
        && health.update == UpdateState::None
        && files::private_binary_hash(lease.program()).map_err(|_| "INSTALL_BINARY_INVALID")?
            == marker.old_hash
    {
        marker.phase = HandoverPhase::RolledBack;
        marker_write(lease, marker).await?;
        return Ok(());
    }
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
    Ok(())
}
