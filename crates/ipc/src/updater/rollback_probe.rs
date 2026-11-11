//! Read-only availability checks use an isolated copy, never stop the live daemon.
use super::{check_state, terminal};
use crate::{
    BuildFlavor,
    installation::{self as files, Handover, HandoverPhase},
    private,
    recovery::{RecoveryLatch, RecoveryPhase, state_hash},
};
use std::{
    io,
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static PROBE_ID: AtomicU64 = AtomicU64::new(0);

/// A positive result is point-in-time evidence, not authority to skip the final stopped-state check.
pub fn checked_restored_rollback(
    flavor: BuildFlavor,
    root: &Path,
    program: &Path,
) -> io::Result<Option<Handover>> {
    checked_with(flavor, root, program, probe_copy)
}

fn checked_with(
    flavor: BuildFlavor,
    root: &Path,
    program: &Path,
    probe: impl FnOnce(BuildFlavor, &Path, &Path, &str) -> io::Result<()>,
) -> io::Result<Option<Handover>> {
    let Some(latch) = RecoveryLatch::read(root)?.filter(|l| l.phase == RecoveryPhase::Restored)
    else {
        return Ok(None);
    };
    let previous = program.with_extension("prev");
    match std::fs::symlink_metadata(&previous) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let old_hash = files::private_binary_hash(&previous)?;
    let target_hash = files::private_binary_hash(program)?;
    if old_hash == target_hash || target_hash != latch.failed_hash || !restored_bytes(root, &latch)?
    {
        return Ok(None);
    }
    let handover = Handover::read(root)?;
    if handover.as_ref().is_some_and(|marker| {
        !terminal(marker.phase)
            && (marker.target_hash == latch.failed_hash || latch.allows_retry(marker))
            && (marker.old_hash != old_hash || marker.target_hash != target_hash)
    }) {
        return Ok(None);
    }
    if probe(
        flavor,
        root,
        &previous,
        latch.restored_hash.as_ref().unwrap(),
    )
    .is_err()
    {
        return Ok(None);
    }
    if RecoveryLatch::read(root)?.as_ref() != Some(&latch)
        || Handover::read(root)? != handover
        || files::private_binary_hash(&previous)? != old_hash
        || files::private_binary_hash(program)? != target_hash
        || !restored_bytes(root, &latch)?
    {
        return Ok(None);
    }
    Ok(Some(Handover {
        old_hash,
        target_hash,
        phase: HandoverPhase::RecoveryRequired,
        migration: None,
    }))
}

pub(super) fn restored_bytes(root: &Path, latch: &RecoveryLatch) -> io::Result<bool> {
    for suffix in ["-wal", "-shm", "-journal"] {
        match private::existing_file(&root.join(format!("state.sqlite3{suffix}"))) {
            Ok(file) if file.metadata()?.len() == 0 => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(latch.restored_hash.as_deref() == Some(state_hash(&root.join("state.sqlite3"))?.as_str()))
}

fn probe_copy(flavor: BuildFlavor, root: &Path, program: &Path, expected: &str) -> io::Result<()> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let scratch = root.join(format!(
        ".rollback-check-{}-{stamp}-{}",
        std::process::id(),
        PROBE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    match std::fs::symlink_metadata(&scratch) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => return Err(io::Error::other("ROLLBACK_CHECK_EXISTS")),
    }
    let guard = private::directory(&scratch)?;
    let result = (|| {
        #[cfg(target_os = "macos")]
        let (_library, _support, base) = {
            let library = private::directory(&scratch.join("Library"))?;
            let support = private::directory(&scratch.join("Library/Application Support"))?;
            (
                library,
                support,
                scratch.join("Library/Application Support"),
            )
        };
        #[cfg(not(target_os = "macos"))]
        let base = scratch.clone();
        let copy_root = base.join(flavor.directory_name());
        let _guard = private::directory(&copy_root)?;
        drop(private::file(
            &copy_root.join(format!("{}.lock", flavor.daemon_name())),
        )?);
        let copy = copy_root.join("state.sqlite3");
        let mut source = private::existing_file(&root.join("state.sqlite3"))?;
        let mut output = private::file(&copy)?;
        io::copy(&mut source, &mut output)?;
        output.sync_all()?;
        drop(output);
        if state_hash(&copy)? != expected {
            return Err(io::Error::other("ROLLBACK_STATE_CHANGED"));
        }
        let mut command = Command::new(program);
        command.env_clear().current_dir(&scratch);
        for key in [
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
            "TMPDIR",
            "TMP",
            "TEMP",
        ] {
            command.env(key, &scratch);
        }
        #[cfg(windows)]
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
        }
        check_state(command)?;
        if state_hash(&copy)? != expected {
            return Err(io::Error::other("ROLLBACK_CHECK_MODIFIED_COPY"));
        }
        Ok(())
    })();
    drop(guard);
    // Only this call's freshly created private scratch directory is removed.
    let cleanup = std::fs::remove_dir_all(scratch);
    result.and(cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        RecoveryLatch,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        drop(private::directory(&root).unwrap());
        drop(private::directory(&root.join("bin")).unwrap());
        let program = root.join("bin/daemon.exe");
        private::file(&program)
            .unwrap()
            .write_all(b"current synthetic binary")
            .unwrap();
        private::file(&program.with_extension("prev"))
            .unwrap()
            .write_all(b"previous synthetic binary")
            .unwrap();
        private::file(&root.join("state.sqlite3"))
            .unwrap()
            .write_all(b"synthetic restored bytes")
            .unwrap();
        let latch = RecoveryLatch {
            phase: RecoveryPhase::Restored,
            from_schema: 5,
            to_schema: 6,
            failed_hash: files::private_binary_hash(&program).unwrap(),
            backup_file: Some(format!("state.sqlite3.v5.{}.bak", "a".repeat(32))),
            restored_hash: Some(state_hash(&root.join("state.sqlite3")).unwrap()),
            failed_state: Some(format!("state.sqlite3.failed.{}", "a".repeat(32))),
            retry: None,
        };
        latch.write(&root).unwrap();
        (temp, root, program, latch)
    }

    #[test]
    fn availability_requires_proof_and_does_not_require_a_handover_marker() {
        let (_temp, root, program, _) = fixture();
        assert!(Handover::read(&root).unwrap().is_none());
        let before = std::fs::read(root.join("state.sqlite3")).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| Err(
                io::Error::other("incompatible")
            ))
            .unwrap()
            .is_none()
        );
        let marker = checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(
            marker.old_hash,
            files::private_binary_hash(&program.with_extension("prev")).unwrap()
        );
        assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), before);
        assert!(Handover::read(&root).unwrap().is_none());
        std::fs::remove_file(program.with_extension("prev")).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| panic!(
                "missing binary must not execute"
            ))
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn an_active_marker_must_name_the_retained_binary() {
        let (_temp, root, program, latch) = fixture();
        let mut marker = Handover {
            old_hash: "a".repeat(64),
            target_hash: latch.failed_hash,
            phase: HandoverPhase::RecoveryRequired,
            migration: None,
        };
        marker.write(&root).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| panic!(
                "wrong previous executable must not execute"
            ))
            .unwrap()
            .is_none()
        );
        marker.old_hash = files::private_binary_hash(&program.with_extension("prev")).unwrap();
        marker.write(&root).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| Ok(()))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn changed_state_or_binary_and_nonempty_sidecars_invalidate_proof() {
        for change in ["state", "previous", "latch", "marker", "wal"] {
            let (_temp, root, program, mut latch) = fixture();
            let result = checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| {
                match change {
                    "state" => private::file(&root.join("state.sqlite3"))?.write_all(b"changed")?,
                    "previous" => {
                        private::file(&program.with_extension("prev"))?.write_all(b"changed")?
                    }
                    "marker" => Handover {
                        old_hash: "a".repeat(64),
                        target_hash: latch.failed_hash.clone(),
                        phase: HandoverPhase::RecoveryRequired,
                        migration: None,
                    }
                    .write(&root)?,
                    "latch" => {
                        latch.phase = RecoveryPhase::Failed;
                        latch.write(&root)?;
                    }
                    _ => private::file(&root.join("state.sqlite3-wal"))?.write_all(b"changed")?,
                }
                Ok(())
            });
            assert!(result.unwrap().is_none(), "{change}");
        }
        let (_temp, root, program, mut latch) = fixture();
        latch.phase = RecoveryPhase::Failed;
        latch.write(&root).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| panic!(
                "failed migration must not probe"
            ))
            .unwrap()
            .is_none()
        );
    }
}
