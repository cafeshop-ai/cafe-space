//! Read-only availability checks use an isolated copy, never stop the live daemon.
use super::{check_state, terminal};
use crate::{
    BuildFlavor,
    installation::{self as files, Handover, HandoverPhase},
    private,
    recovery::{RecoveryLatch, RecoveryPhase, state_hash},
};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    io,
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static PROBE_ID: AtomicU64 = AtomicU64::new(0);
const STATE_SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];
type Fingerprint = [Option<String>; 4];

/// A positive result is point-in-time evidence, not authority to skip the final stopped-state check.
pub fn checked_recovery_rollback(
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
    probe: impl FnOnce(BuildFlavor, &Path, &Path, &Fingerprint) -> io::Result<()>,
) -> io::Result<Option<Handover>> {
    let latch = RecoveryLatch::read(root)?;
    let handover = Handover::read(root)?;
    if latch
        .as_ref()
        .is_some_and(|l| l.phase == RecoveryPhase::Restoring)
        || (latch.is_none()
            && !handover
                .as_ref()
                .is_some_and(|m| m.phase == HandoverPhase::RecoveryRequired))
    {
        return Ok(None);
    }
    let _root = private::existing_directory(root)?;
    let previous = program.with_extension("prev");
    match std::fs::symlink_metadata(&previous) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    let old_hash = files::private_binary_hash(&previous)?;
    let target_hash = files::private_binary_hash(program)?;
    let retry = handover
        .as_ref()
        .filter(|marker| latch.as_ref().is_some_and(|l| l.allows_retry(marker)));
    let expected_target = retry
        .map(|m| m.target_hash.as_str())
        .or_else(|| latch.as_ref().map(|l| l.failed_hash.as_str()))
        .or_else(|| handover.as_ref().map(|m| m.target_hash.as_str()));
    if old_hash == target_hash
        || expected_target != Some(target_hash.as_str())
        || !matches_restored(root, latch.as_ref(), handover.as_ref())?
        || handover.as_ref().is_some_and(|marker| {
            !terminal(marker.phase)
                && (marker.old_hash != old_hash || marker.target_hash != target_hash)
        })
    {
        return Ok(None);
    }
    let state = fingerprint(root)?;
    if probe(flavor, root, &previous, &state).is_err() {
        return Ok(None);
    }
    if RecoveryLatch::read(root)? != latch
        || Handover::read(root)? != handover
        || files::private_binary_hash(&previous)? != old_hash
        || files::private_binary_hash(program)? != target_hash
        || fingerprint(root)? != state
        || !matches_restored(root, latch.as_ref(), handover.as_ref())?
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

pub(super) fn matches_restored(
    root: &Path,
    latch: Option<&RecoveryLatch>,
    marker: Option<&Handover>,
) -> io::Result<bool> {
    let Some(latch) = latch.filter(|l| {
        l.phase == RecoveryPhase::Restored && !marker.is_some_and(|m| l.allows_retry(m))
    }) else {
        return Ok(true);
    };
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

pub(super) fn fingerprint(root: &Path) -> io::Result<Fingerprint> {
    let mut hashes = [None, None, None, None];
    for (index, suffix) in STATE_SUFFIXES.iter().enumerate() {
        hashes[index] = match state_hash(&root.join(format!("state.sqlite3{suffix}"))) {
            Ok(hash) => Some(hash),
            Err(error) if index != 0 && error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
    }
    Ok(hashes)
}

fn snapshot(root: &Path, raw: &Path, destination: &Path, expected: &Fingerprint) -> io::Result<()> {
    let _guard = private::directory(raw)?;
    for (suffix, hash) in STATE_SUFFIXES.iter().zip(expected) {
        if let Some(hash) = hash
            && *suffix != "-shm"
        {
            let name = format!("state.sqlite3{suffix}");
            let mut source = private::existing_file(&root.join(&name))?;
            let mut copy = private::file(&raw.join(&name))?;
            io::copy(&mut source, &mut copy)?;
            copy.sync_all()?;
            drop(copy);
            if state_hash(&raw.join(name))? != *hash {
                return Err(io::Error::other("ROLLBACK_STATE_CHANGED"));
            }
        }
    }
    if fingerprint(root)? != *expected {
        return Err(io::Error::other("ROLLBACK_STATE_CHANGED"));
    }
    // Rebuild SHM and recover journals only in our copy; never open the live database with SQLite.
    for suffix in ["-wal", "-shm", "-journal"] {
        drop(private::file(&raw.join(format!("state.sqlite3{suffix}")))?);
        drop(private::file(
            &destination.with_file_name(format!("state.sqlite3{suffix}")),
        )?);
    }
    drop(private::file(destination)?);
    let source = Connection::open_with_flags(
        raw.join("state.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    let mut output = Connection::open_with_flags(
        destination,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(io::Error::other)?;
    output
        .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
        .map_err(io::Error::other)?;
    {
        let backup = Backup::new(&source, &mut output).map_err(io::Error::other)?;
        if !matches!(backup.step(-1).map_err(io::Error::other)?, StepResult::Done) {
            return Err(io::Error::other("ROLLBACK_SNAPSHOT_INCOMPLETE"));
        }
    }
    output
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .map_err(io::Error::other)?;
    Ok(())
}

fn probe_copy(
    flavor: BuildFlavor,
    root: &Path,
    program: &Path,
    expected: &Fingerprint,
) -> io::Result<()> {
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
        snapshot(root, &scratch.join("source"), &copy, expected)?;
        let copied = fingerprint(&copy_root)?;
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
        if fingerprint(&copy_root)? != copied {
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
    fn failed_state_is_probed_without_a_backup_and_changes_invalidate_proof() {
        let (_temp, root, program, mut latch) = fixture();
        latch.phase = RecoveryPhase::Failed;
        latch.restored_hash = None;
        latch.backup_file = None;
        latch.failed_state = None;
        latch.write(&root).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| Ok(()))
                .unwrap()
                .is_some()
        );
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| Err(
                io::Error::other("incompatible")
            ))
            .unwrap()
            .is_none()
        );
        for suffix in ["", "-wal", "-journal"] {
            assert!(
                checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| {
                    private::file(&root.join(format!("state.sqlite3{suffix}")))?
                        .write_all(b"changed")
                })
                .unwrap()
                .is_none()
            );
        }
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
        latch.phase = RecoveryPhase::Restoring;
        latch.write(&root).unwrap();
        assert!(
            checked_with(BuildFlavor::Dev, &root, &program, |_, _, _, _| panic!(
                "incomplete restore must not probe"
            ))
            .unwrap()
            .is_none()
        );
    }
}
