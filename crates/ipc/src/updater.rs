//! System-managed handover recovery shared by the desktop and headless CLI.
use crate::{
    BuildFlavor,
    client::ControlClient as Client,
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
mod rollback_probe;
pub use rollback_probe::checked_recovery_rollback;

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
    let recovery = crate::recovery::RecoveryLatch::read(lease.root())
        .map_err(|_| "INSTALL_RECOVERY_INVALID")?;
    let existing = Handover::read(lease.root()).map_err(|_| "INSTALL_MARKER_INVALID")?;
    let marker = if let Some(latch) = recovery
        .as_ref()
        .filter(|_| action == UpdateRecoveryAction::Rollback)
    {
        if latch.phase == crate::recovery::RecoveryPhase::Restoring {
            return Err("INSTALL_EXPLICIT_RESTORE_REQUIRED");
        }
        // Resume interrupted binary swaps even when .prev has already become the installed executable.
        if let Some(marker) = existing.filter(|marker| {
            !terminal(marker.phase)
                && (marker.target_hash == latch.failed_hash || latch.allows_retry(marker))
        }) {
            marker
        } else {
            let refused = rollback_refusal(lease.root(), Some(latch), None);
            let copy = lease.clone();
            let marker = blocking(
                move || checked_recovery_rollback(flavor, copy.root(), copy.program()),
                refused,
            )
            .await?
            .ok_or(refused)?;
            marker_write(&lease, marker.clone()).await?;
            marker
        }
    } else {
        let marker = existing.ok_or("INSTALL_NO_RECOVERY")?;
        if terminal(marker.phase) {
            return Err("INSTALL_NO_RECOVERY");
        }
        marker
    };
    match action {
        UpdateRecoveryAction::Finish => finish(flavor, &lease, marker).await,
        UpdateRecoveryAction::Rollback => rollback(flavor, &lease, marker).await,
    }
}

async fn finish(flavor: BuildFlavor, lease: &Arc<Lease>, mut marker: Handover) -> Result<()> {
    if marker.phase == HandoverPhase::RecoveryRequired
        || crate::recovery::RecoveryLatch::read(lease.root())
            .map_err(|_| "INSTALL_RECOVERY_INVALID")?
            .is_some_and(|latch| !latch.allows_retry(&marker))
    {
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
fn check_state(mut command: Command) -> io::Result<()> {
    let mut child = command
        .arg("--check-state")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let until = std::time::Instant::now() + Duration::from_secs(15);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("old binary refused current state"))
                };
            }
            Err(error) => break Err(error),
            Ok(None) if std::time::Instant::now() >= until => {
                break Err(io::Error::other("old state check timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn rollback_refusal(
    root: &Path,
    latch: Option<&crate::recovery::RecoveryLatch>,
    marker: Option<&Handover>,
) -> &'static str {
    let backup = match latch {
        Some(latch) => latch.backup_file.as_ref(),
        None => marker
            .and_then(|m| m.migration.as_ref())
            .map(|m| &m.backup_file),
    };
    if backup.is_some_and(|name| crate::private::existing_file(&root.join(name)).is_ok()) {
        "INSTALL_EXPLICIT_RESTORE_REQUIRED"
    } else {
        "INSTALL_ROLLBACK_STATE_REFUSED"
    }
}

pub async fn rollback(flavor: BuildFlavor, lease: &Arc<Lease>, mut marker: Handover) -> Result<()> {
    let recovery = crate::recovery::RecoveryLatch::read(lease.root())
        .map_err(|_| "INSTALL_RECOVERY_INVALID")?;
    if recovery
        .as_ref()
        .is_some_and(|latch| latch.phase == crate::recovery::RecoveryPhase::Restoring)
    {
        return Err("INSTALL_EXPLICIT_RESTORE_REQUIRED");
    }
    if let Some(current) = Handover::read(lease.root()).map_err(|_| "INSTALL_MARKER_INVALID")? {
        if current.old_hash != marker.old_hash || current.target_hash != marker.target_hash {
            return Err("INSTALL_HANDOVER_CHANGED");
        }
        marker.migration = current.migration;
    }
    if let Ok((_, health)) = Client::connect(flavor, lease.root()).await
        && health.binary_hash.as_deref() == Some(&marker.old_hash)
        && health.ready
        && health.update == UpdateState::None
        && files::private_binary_hash(lease.program()).map_err(|_| "INSTALL_BINARY_INVALID")?
            == marker.old_hash
    {
        marker.phase = HandoverPhase::RolledBack;
        marker.migration = None;
        marker_write(lease, marker).await?;
        return Ok(());
    }
    let refused = rollback_refusal(lease.root(), recovery.as_ref(), Some(&marker));
    let restored = async {
        if (recovery.is_some() || marker.phase == HandoverPhase::RecoveryRequired)
            && files::private_binary_hash(lease.program()).map_err(|_| "INSTALL_BINARY_INVALID")?
                != marker.old_hash
        {
            let copy = lease.clone();
            if blocking(
                move || checked_recovery_rollback(flavor, copy.root(), copy.program()),
                refused,
            )
            .await?
            .is_none()
            {
                return Err(refused);
            }
        }
        system(lease, Action::Verify).await?;
        system(lease, Action::Stop).await?;
        drop(stopped_guard(flavor, lease.root()).await?);
        // Health may have committed the migration since rollback first read the marker.
        if let Some(current) = Handover::read(lease.root()).map_err(|_| "INSTALL_MARKER_INVALID")? {
            if current.old_hash != marker.old_hash || current.target_hash != marker.target_hash {
                return Err("INSTALL_HANDOVER_CHANGED");
            }
            marker.migration = current.migration;
        }
        let program = match files::private_binary_hash(lease.program()) {
            Ok(hash) if hash == marker.old_hash => lease.program().to_owned(),
            _ => lease.program().with_extension("prev"),
        };
        let old = marker.old_hash.clone();
        let root = lease.root().to_owned();
        let checked_marker = marker.clone();
        let checked_state = blocking(
            move || {
                if crate::recovery::RecoveryLatch::read(&root)? != recovery
                    || !rollback_probe::matches_restored(
                        &root,
                        recovery.as_ref(),
                        Some(&checked_marker),
                    )?
                {
                    return Err(io::Error::other("recovery state changed"));
                }
                if files::private_binary_hash(&program)? != old {
                    return Err(io::Error::other("old binary hash mismatch"));
                }
                let before = rollback_probe::fingerprint(&root)?;
                check_state(Command::new(program))?;
                if rollback_probe::fingerprint(&root)? != before {
                    return Err(io::Error::other("old binary modified state"));
                }
                Ok(before)
            },
            refused,
        )
        .await?;
        let stopped = stopped_guard(flavor, lease.root()).await?;
        let copy = lease.clone();
        let old = marker.old_hash.clone();
        let target = marker.target_hash.clone();
        blocking(
            move || {
                if rollback_probe::fingerprint(copy.root())? != checked_state {
                    return Err(io::Error::other("state changed after check"));
                }
                files::restore_previous(copy.program(), &old, &target)
            },
            "INSTALL_ROLLBACK_BINARY_FAILED",
        )
        .await?;
        marker.phase = HandoverPhase::RolledBack;
        marker.migration = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompatible_state_needs_a_retained_backup_to_offer_restore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        drop(crate::private::directory(&root).unwrap());
        let name = format!("state.sqlite3.v1.{}.bak", "a".repeat(32));
        let latch = crate::recovery::RecoveryLatch {
            phase: crate::recovery::RecoveryPhase::Failed,
            from_schema: 1,
            to_schema: files::STATE_SCHEMA_VERSION,
            failed_hash: "b".repeat(64),
            backup_file: Some(name.clone()),
            restored_hash: None,
            failed_state: None,
            retry: None,
        };
        let marker = Handover {
            old_hash: "a".repeat(64),
            target_hash: latch.failed_hash.clone(),
            phase: HandoverPhase::RecoveryRequired,
            migration: Some(files::SchemaMigration {
                from_version: 1,
                to_version: files::STATE_SCHEMA_VERSION,
                backup_file: name.clone(),
                created_at: 1,
                completed: true,
            }),
        };
        for present in [false, true] {
            if present {
                drop(crate::private::file(&root.join(&name)).unwrap());
            }
            let code = if present {
                "INSTALL_EXPLICIT_RESTORE_REQUIRED"
            } else {
                "INSTALL_ROLLBACK_STATE_REFUSED"
            };
            assert_eq!(rollback_refusal(&root, Some(&latch), None), code);
            assert_eq!(rollback_refusal(&root, None, Some(&marker)), code);
        }
        let mut no_backup = latch;
        no_backup.backup_file = None;
        assert_eq!(
            rollback_refusal(&root, Some(&no_backup), Some(&marker)),
            "INSTALL_ROLLBACK_STATE_REFUSED"
        );
    }
}
