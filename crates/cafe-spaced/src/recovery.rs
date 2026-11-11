//! Owner-only restore while the daemon retains its instance lock and admission fence.
use crate::state::State;
use cafe_space_ipc::{
    installation::{Handover, HandoverPhase},
    private,
    recovery::{LATCH_FILE, RecoveryLatch, RecoveryPhase, state_hash},
};
use cafe_space_protocol::{RecoveryCode, RecoveryDetails, RecoveryState};
use rusqlite::{Connection, OpenFlags};
use std::{io, path::Path};

const SUFFIXES: [&str; 4] = ["", "-wal", "-shm", "-journal"];
const APPLICATION_ID: i32 = 0x43414645;

#[cfg(test)]
mod tests;

pub(crate) fn load(root: &Path, marker: Option<&Handover>) -> io::Result<Option<RecoveryLatch>> {
    let mut latch = match RecoveryLatch::read(root)? {
        Some(latch) => latch,
        None => {
            let Some(marker) = marker.filter(|m| m.phase == HandoverPhase::RecoveryRequired) else {
                return Ok(None);
            };
            let migration = marker
                .migration
                .as_ref()
                .ok_or_else(|| io::Error::other("RECOVERY_METADATA_UNAVAILABLE"))?;
            let latch = RecoveryLatch {
                phase: RecoveryPhase::Failed,
                from_schema: migration.from_version,
                to_schema: migration.to_version,
                failed_hash: marker.target_hash.clone(),
                backup_file: Some(migration.backup_file.clone()),
                restored_hash: None,
                failed_state: None,
                retry: None,
            };
            latch.write(root)?;
            latch
        }
    };
    if latch.phase == RecoveryPhase::Restoring {
        // A schema number alone cannot distinguish the untouched failed DB from the restored copy.
        let restored = latch.restored_hash.as_ref().is_some_and(|expected| {
            let path = root.join("state.sqlite3");
            latch
                .failed_state
                .as_ref()
                .is_some_and(|name| private::existing_file(&root.join(name)).is_ok())
                && SUFFIXES[1..].iter().all(|suffix| {
                    match private::existing_file(&root.join(format!("state.sqlite3{suffix}"))) {
                        Ok(file) => file.metadata().is_ok_and(|m| m.len() == 0),
                        Err(e) => e.kind() == io::ErrorKind::NotFound,
                    }
                })
                && state_hash(&path).is_ok_and(|hash| hash == *expected)
                && validated_database(&path, latch.from_schema).is_ok()
        });
        latch.phase = if restored {
            RecoveryPhase::Restored
        } else {
            RecoveryPhase::Failed
        };
        latch.write(root)?;
    }
    Ok(Some(latch))
}

pub(crate) fn status(root: &Path, latch: &RecoveryLatch) -> RecoveryState {
    RecoveryState {
        code: if latch.phase == RecoveryPhase::Restored {
            RecoveryCode::RollbackRequired
        } else {
            RecoveryCode::SchemaMigrationFailed
        },
        from_schema: latch.from_schema,
        to_schema: latch.to_schema,
        backup: latch
            .backup_file
            .as_ref()
            .is_some_and(|name| private::existing_file(&root.join(name)).is_ok()),
    }
}

pub(crate) fn details(root: &Path, latch: &RecoveryLatch) -> io::Result<RecoveryDetails> {
    let text = |path: &Path| {
        path.to_str()
            .filter(|value| {
                !value.is_empty() && value.len() <= 65_536 && !value.chars().any(char::is_control)
            })
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other("RECOVERY_PATH_INVALID"))
    };
    let mut files = vec![
        text(&root.join("state.sqlite3"))?,
        text(&root.join(LATCH_FILE))?,
    ];
    for name in [
        "state.sqlite3-wal",
        "state.sqlite3-shm",
        "state.sqlite3-journal",
        "handover.json",
    ] {
        let path = root.join(name);
        if private::existing_file(&path).is_ok() {
            files.push(text(&path)?);
        }
    }
    if let Some(name) = &latch.failed_state {
        for suffix in SUFFIXES {
            let path = root.join(format!("{name}{suffix}"));
            if private::existing_file(&path).is_ok() {
                files.push(text(&path)?);
            }
        }
    }
    Ok(RecoveryDetails {
        data_directory: text(root)?,
        backup_file: latch
            .backup_file
            .as_ref()
            .filter(|name| private::existing_file(&root.join(name)).is_ok())
            .map(|name| text(&root.join(name)))
            .transpose()?,
        state_files: files,
    })
}

fn validated_database(path: &Path, version: u32) -> io::Result<Connection> {
    let _file = private::existing_file(path)?;
    use std::fmt::Write as _;
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_encoded_bytes() {
        write!(&mut uri, "%{byte:02X}").map_err(io::Error::other)?;
    }
    uri.push_str("?immutable=1");
    let database = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(io::Error::other)?;
    let app: i32 = database
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(io::Error::other)?;
    let schema: u32 = database
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(io::Error::other)?;
    let integrity: String = database
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(io::Error::other)?;
    if app != APPLICATION_ID || schema != version || integrity != "ok" {
        return Err(io::Error::other("RESTORE_BACKUP_INVALID"));
    }
    Ok(database)
}

fn copy_private(source: &Path, target: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(target) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
        Ok(_) => return Err(io::Error::other("RECOVERY_FILE_EXISTS")),
    }
    let mut source = private::existing_file(source)?;
    let mut temporary = target.as_os_str().to_os_string();
    temporary.push(".writing");
    let temporary = std::path::PathBuf::from(temporary);
    if std::fs::symlink_metadata(&temporary).is_ok() {
        return Err(io::Error::other("RECOVERY_FILE_EXISTS"));
    }
    let mut output = private::file(&temporary)?;
    let result = io::copy(&mut source, &mut output).and_then(|_| output.sync_all());
    drop(output);
    result?;
    private::replace_durable(&temporary, target)
}

#[derive(Debug)]
pub(crate) enum RestoreError {
    Rejected(&'static str),
    Unknown,
}

pub(crate) fn restore(state: &mut State, root: &Path) -> Result<(), RestoreError> {
    let mut latch = state
        .recovery
        .clone()
        .ok_or(RestoreError::Rejected("RESTORE_NOT_AVAILABLE"))?;
    if latch.phase != RecoveryPhase::Failed || !status(root, &latch).backup {
        return Err(RestoreError::Rejected("RESTORE_NOT_AVAILABLE"));
    }
    let backup = root.join(
        latch
            .backup_file
            .as_ref()
            .ok_or(RestoreError::Rejected("RESTORE_NOT_AVAILABLE"))?,
    );
    let _guard = private::existing_file(&backup)
        .map_err(|_| RestoreError::Rejected("RESTORE_BACKUP_INVALID"))?;
    drop(
        validated_database(&backup, latch.from_schema)
            .map_err(|_| RestoreError::Rejected("RESTORE_BACKUP_INVALID"))?,
    );
    let hash = state_hash(&backup).map_err(|_| RestoreError::Rejected("RESTORE_BACKUP_INVALID"))?;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| RestoreError::Rejected("RESTORE_PREPARATION_FAILED"))?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = root.join(format!("state.sqlite3.restore.{nonce}"));
    copy_private(&backup, &temporary)
        .map_err(|_| RestoreError::Rejected("RESTORE_PREPARATION_FAILED"))?;
    if state_hash(&temporary).map_err(|_| RestoreError::Rejected("RESTORE_BACKUP_INVALID"))? != hash
    {
        return Err(RestoreError::Rejected("RESTORE_BACKUP_INVALID"));
    }
    drop(
        validated_database(&temporary, latch.from_schema)
            .map_err(|_| RestoreError::Rejected("RESTORE_BACKUP_INVALID"))?,
    );
    crate::migrations::require_recovery(root)
        .map_err(|_| RestoreError::Rejected("RESTORE_PREPARATION_FAILED"))?;
    latch.phase = RecoveryPhase::Restoring;
    latch.restored_hash = Some(hash);
    let failed = format!("state.sqlite3.failed.{nonce}");
    latch.failed_state = Some(failed.clone());
    // A failed durable write can have taken effect; never claim rejection after this point.
    state.recovery = Some(latch.clone());
    latch.write(root).map_err(|_| RestoreError::Unknown)?;
    // Snapshot all diagnostic bytes before SQLite's close-time checkpoint can remove its WAL.
    for suffix in SUFFIXES {
        let source = root.join(format!("state.sqlite3{suffix}"));
        match private::existing_file(&source) {
            Ok(file) => drop(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(RestoreError::Unknown),
        }
        copy_private(&source, &root.join(format!("{failed}{suffix}")))
            .map_err(|_| RestoreError::Unknown)?;
    }
    let detached = std::mem::replace(
        &mut state.database,
        Connection::open_in_memory().map_err(|_| RestoreError::Unknown)?,
    );
    if let Err((database, _)) = detached.close() {
        state.database = database;
        return Err(RestoreError::Unknown);
    }
    state.database_file = None;
    for suffix in &SUFFIXES[1..] {
        let source = root.join(format!("state.sqlite3{suffix}"));
        match private::existing_file(&source) {
            Ok(file) => drop(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(RestoreError::Unknown),
        }
        drop(
            private::existing_file(&root.join(format!("{failed}{suffix}")))
                .map_err(|_| RestoreError::Unknown)?,
        );
        std::fs::remove_file(source).map_err(|_| RestoreError::Unknown)?;
    }
    private::replace_durable(&temporary, &root.join("state.sqlite3"))
        .map_err(|_| RestoreError::Unknown)?;
    state.database_file = Some(
        private::existing_file(&root.join("state.sqlite3")).map_err(|_| RestoreError::Unknown)?,
    );
    state.database = validated_database(&root.join("state.sqlite3"), latch.from_schema)
        .map_err(|_| RestoreError::Unknown)?;
    latch.phase = RecoveryPhase::Restored;
    latch.write(root).map_err(|_| RestoreError::Unknown)?;
    state.recovery = Some(latch);
    Ok(())
}
