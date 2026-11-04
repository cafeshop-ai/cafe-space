//! Versioned schema upgrades: one verified starting-version backup, then one transaction.
use cafe_space_ipc::{
    installation::{Handover, HandoverPhase, SchemaMigration},
    private,
};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const SCHEMA_VERSION: i32 = cafe_space_ipc::installation::STATE_SCHEMA_VERSION as i32;

pub(crate) fn run(database: &mut Connection, root: &Path) -> io::Result<()> {
    let result = migrate(database, root);
    if result.is_err() {
        require_recovery(root)?;
    }
    result
}

pub(crate) fn require_recovery(root: &Path) -> io::Result<()> {
    // shortcut: no-marker failures lack a durable latch; add it in M1-8 before real-agent handover.
    if let Some(mut marker) = Handover::read(root)?
        && marker.phase == HandoverPhase::Committed
    {
        marker.phase = HandoverPhase::RecoveryRequired;
        marker.write(root)?;
    }
    Ok(())
}

fn migrate(database: &mut Connection, root: &Path) -> io::Result<()> {
    let version: i32 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(failed)?;
    let mut handover = Handover::read(root)?;
    if handover
        .as_ref()
        .is_some_and(|marker| marker.phase == HandoverPhase::RecoveryRequired)
    {
        return Err(failed("owner recovery required"));
    }
    if version == SCHEMA_VERSION {
        return finish_marker(database, root, handover);
    }
    if !(1..SCHEMA_VERSION).contains(&version) {
        return Err(failed("unsupported schema"));
    }
    if handover.as_ref().is_some_and(|marker| {
        !matches!(
            marker.phase,
            HandoverPhase::Committed | HandoverPhase::NotApplied | HandoverPhase::RolledBack
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "SCHEMA_MIGRATION_NOT_COMMITTED",
        ));
    }
    if let Some(previous) = handover
        .as_ref()
        .and_then(|marker| marker.migration.as_ref())
    {
        if previous.to_version != version as u32 {
            // A backup without its committed schema is an interrupted attempt, not permission to retry.
            return Err(failed("interrupted schema migration"));
        }
        finish_marker(database, root, handover.clone())?;
    }
    let mut record =
        backup(database, root, version).map_err(|_| io::Error::other("SCHEMA_BACKUP_FAILED"))?;
    if let Some(marker) = handover
        .as_mut()
        .filter(|marker| marker.phase == HandoverPhase::Committed)
    {
        marker.migration = Some(record.clone());
        marker.write(root)?;
    }
    {
        let transaction = database.transaction().map_err(failed)?;
        for to in version + 1..=SCHEMA_VERSION {
            let sql = match to {
                2 => include_str!("../../../fixtures/migrations/v2.sql"),
                3 => include_str!("../../../fixtures/migrations/v3.sql"),
                _ => return Err(failed("missing schema migration")),
            };
            transaction.execute_batch(sql).map_err(failed)?;
            transaction.execute(
                "INSERT INTO migration_history (to_version, from_version, backup_file, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![to, to - 1, record.backup_file, record.created_at],
            ).map_err(failed)?;
        }
        transaction.commit().map_err(failed)?;
    }
    record.completed = true;
    if let Some(marker) = handover
        .as_mut()
        .filter(|marker| marker.phase == HandoverPhase::Committed)
    {
        marker.migration = Some(record);
        marker.write(root)?;
    }
    Ok(())
}

fn finish_marker(
    database: &Connection,
    root: &Path,
    mut handover: Option<Handover>,
) -> io::Result<()> {
    let version: u32 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(failed)?;
    // Require provenance even without a handover marker.
    let _: u32 = database
        .query_row(
            "SELECT from_version FROM migration_history WHERE to_version=?1",
            [version],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if let Some(marker) = handover.as_mut()
        && let Some(previous) = marker.migration.as_ref()
    {
        if previous.to_version != version {
            return Err(failed("schema and marker differ"));
        }
        for to in previous.from_version + 1..=previous.to_version {
            let row: (u32, String, i64) = database.query_row(
                "SELECT from_version, backup_file, created_at FROM migration_history WHERE to_version=?1",
                [to], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).map_err(failed)?;
            if row != (to - 1, previous.backup_file.clone(), previous.created_at) {
                return Err(failed("schema and marker differ"));
            }
        }
        if !previous.completed {
            if marker.phase != HandoverPhase::Committed {
                return Err(failed("handover not committed"));
            }
            let mut completed = previous.clone();
            completed.completed = true;
            marker.migration = Some(completed);
            marker.write(root)?;
        }
    }
    Ok(())
}

fn backup(database: &Connection, root: &Path, version: i32) -> io::Result<SchemaMigration> {
    let _root = private::directory(root)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let nonce: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    // A retry keeps earlier recovery data instead of overwriting a failed attempt's backup.
    let name = format!("state.sqlite3.v{version}.{nonce}.bak");
    let target = root.join(&name);
    match std::fs::symlink_metadata(&target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "backup already exists",
            ));
        }
    }
    let temporary = target.with_extension("bak.writing");
    let file = private::file(&temporary)?;
    if file.metadata()?.len() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "backup already exists",
        ));
    }
    let cleanup = temporary.clone();
    let result = (move || {
        let mut destination = Connection::open_with_flags(
            &temporary,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(io::Error::other)?;
        destination
            .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
            .map_err(io::Error::other)?;
        {
            let copy = Backup::new(database, &mut destination).map_err(io::Error::other)?;
            if !matches!(copy.step(-1).map_err(io::Error::other)?, StepResult::Done) {
                return Err(io::Error::other("backup did not complete"));
            }
        }
        destination
            .execute_batch("PRAGMA journal_mode=DELETE;")
            .map_err(io::Error::other)?;
        let integrity: String = destination
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        if integrity != "ok" {
            return Err(io::Error::other("backup integrity failed"));
        }
        drop(destination);
        file.sync_all()?;
        drop(file);
        private::replace_durable(&temporary, &target)?;
        Ok(SchemaMigration {
            from_version: version as u32,
            to_version: SCHEMA_VERSION as u32,
            backup_file: name,
            created_at: i64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(io::Error::other)?
                    .as_secs(),
            )
            .map_err(io::Error::other)?,
            completed: false,
        })
    })();
    if result.is_err() {
        // All SQLite/file handles are closed before cleanup, including on Windows.
        let _ = std::fs::remove_file(&cleanup);
    }
    result
}

fn failed(_: impl std::fmt::Display) -> io::Error {
    io::Error::other("SCHEMA_MIGRATION_FAILED")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        drop(private::directory(&root).unwrap());
        let database = Connection::open_in_memory().unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v1.sql"))
            .unwrap();
        database.pragma_update(None, "foreign_keys", true).unwrap();
        database
            .busy_timeout(std::time::Duration::from_millis(20))
            .unwrap();
        (temp, root, database)
    }
    #[test]
    fn consistent_backup_precedes_upgrade_and_is_retained_on_failure() {
        for conflict in [false, true] {
            let (_temp, root, mut database) = fixture();
            if conflict {
                database
                    .execute("CREATE TABLE migration_history (sentinel TEXT)", [])
                    .unwrap();
            }
            let result = run(&mut database, &root);
            assert_eq!(result.is_err(), conflict);
            assert_eq!(
                database
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                    .unwrap(),
                if conflict { 1 } else { SCHEMA_VERSION }
            );
            let backups: Vec<_> = std::fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.extension().is_some_and(|value| value == "bak"))
                .collect();
            assert_eq!(backups.len(), 1);
            let backup =
                Connection::open_with_flags(&backups[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            assert_eq!(
                backup
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .unwrap(),
                "ok"
            );
            assert_eq!(
                backup
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                backup
                    .query_row("SELECT value FROM legacy_probe WHERE id=2", [], |row| {
                        row.get::<_, String>(0)
                    })
                    .unwrap(),
                "Résumé 茶"
            );
            assert_eq!(
                database
                    .query_row("SELECT value FROM legacy_probe WHERE id=1", [], |row| {
                        row.get::<_, String>(0)
                    })
                    .unwrap(),
                "Fixture preserved"
            );
            if conflict {
                assert_eq!(
                    database
                        .query_row(
                            "SELECT count(*) FROM sqlite_schema WHERE name='sessions'",
                            [],
                            |row| row.get::<_, i32>(0)
                        )
                        .unwrap(),
                    0
                );
            }
            if !conflict {
                run(&mut database, &root).unwrap();
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
            }
        }
    }
    #[test]
    fn upgrades_each_supported_start_in_one_transaction_and_one_backup() {
        for from in [1, 2] {
            for conflict in [false, true] {
                let (_temp, root, mut database) = fixture();
                if from == 2 {
                    database
                        .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
                        .unwrap();
                    database
                        .execute(
                            "INSERT INTO sessions VALUES ('sentinel','original','preserved')",
                            [],
                        )
                        .unwrap();
                }
                if conflict {
                    database
                        .execute("CREATE TABLE projects (sentinel TEXT)", [])
                        .unwrap();
                }
                Handover {
                    old_hash: "a".repeat(64),
                    target_hash: "b".repeat(64),
                    phase: HandoverPhase::Committed,
                    migration: None,
                }
                .write(&root)
                .unwrap();
                assert_eq!(run(&mut database, &root).is_err(), conflict);
                let marker = Handover::read(&root).unwrap().unwrap();
                let record = marker.migration.as_ref().unwrap();
                assert_eq!((record.from_version, record.to_version), (from, 3));
                assert_eq!(record.completed, !conflict);
                assert!(
                    record
                        .backup_file
                        .starts_with(&format!("state.sqlite3.v{from}."))
                );
                let backups: Vec<_> = std::fs::read_dir(&root)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|p| p.extension().is_some_and(|e| e == "bak"))
                    .collect();
                assert_eq!(backups.len(), 1);
                let backup =
                    Connection::open_with_flags(&backups[0], OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .unwrap();
                assert_eq!(
                    backup
                        .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                        .unwrap(),
                    from
                );
                assert_eq!(
                    database
                        .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                        .unwrap(),
                    if conflict { from } else { 3 }
                );
                if from == 2 {
                    assert_eq!(
                        database
                            .query_row(
                                "SELECT snapshot FROM sessions WHERE id='sentinel'",
                                [],
                                |r| r.get::<_, String>(0)
                            )
                            .unwrap(),
                        "preserved"
                    );
                }
                if conflict {
                    assert_eq!(marker.phase, HandoverPhase::RecoveryRequired);
                    if from == 1 {
                        assert_eq!(
                            database
                                .query_row(
                                    "SELECT count(*) FROM sqlite_schema WHERE name='sessions'",
                                    [],
                                    |r| r.get::<_, u32>(0)
                                )
                                .unwrap(),
                            0
                        );
                    }
                    assert!(run(&mut database, &root).is_err());
                } else {
                    for to in from + 1..=3 {
                        let row: (u32, String) = database.query_row("SELECT from_version,backup_file FROM migration_history WHERE to_version=?1", [to], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
                        assert_eq!(row, (to - 1, record.backup_file.clone()));
                    }
                    run(&mut database, &root).unwrap();
                }
                assert_eq!(
                    std::fs::read_dir(&root).unwrap().count(),
                    2,
                    "one marker and one backup, no repeated backup or .writing"
                );
            }
        }
    }

    #[test]
    fn completed_v2_marker_is_verified_before_a_new_backup_and_upgrade() {
        for corrupt in [false, true] {
            let (_temp, root, mut database) = fixture();
            database
                .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
                .unwrap();
            let name = format!("state.sqlite3.v1.{}.bak", "a".repeat(32));
            database
                .execute("INSERT INTO migration_history VALUES (2,1,?1,1)", [&name])
                .unwrap();
            Handover {
                old_hash: "a".repeat(64),
                target_hash: "b".repeat(64),
                phase: HandoverPhase::Committed,
                migration: Some(SchemaMigration {
                    from_version: 1,
                    to_version: 2,
                    backup_file: name,
                    created_at: if corrupt { 2 } else { 1 },
                    completed: true,
                }),
            }
            .write(&root)
            .unwrap();
            assert_eq!(run(&mut database, &root).is_err(), corrupt);
            let marker = Handover::read(&root).unwrap().unwrap();
            let record = marker.migration.unwrap();
            if corrupt {
                assert_eq!(marker.phase, HandoverPhase::RecoveryRequired);
                assert_eq!(record.to_version, 2);
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
            } else {
                assert_eq!(
                    (record.from_version, record.to_version, record.completed),
                    (2, 3, true)
                );
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
                run(&mut database, &root).unwrap();
            }
        }
    }

    #[test]
    fn backup_failure_does_not_begin_the_schema_transaction() {
        let (_temp, root, mut database) = fixture();
        database
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO legacy_probe VALUES (3, 'uncommitted');")
            .unwrap();
        let error = run(&mut database, &root).unwrap_err();
        assert_eq!(error.to_string(), "SCHEMA_BACKUP_FAILED");
        assert_eq!(
            database
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            1
        );
        database.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            database
                .query_row("SELECT count(*) FROM legacy_probe", [], |row| row
                    .get::<_, i32>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            0,
            "failed backup must leave neither .bak nor .writing files"
        );
    }

    #[test]
    fn committed_failures_latch_recovery_and_preserve_the_last_backup_record() {
        for backup_fails in [true, false] {
            let (_temp, root, mut database) = fixture();
            Handover {
                old_hash: "a".repeat(64),
                target_hash: "b".repeat(64),
                phase: HandoverPhase::Committed,
                migration: None,
            }
            .write(&root)
            .unwrap();
            database
                .execute_batch(if backup_fails {
                    "BEGIN IMMEDIATE; INSERT INTO legacy_probe VALUES (3, 'uncommitted');"
                } else {
                    "CREATE TABLE migration_history (sentinel TEXT);"
                })
                .unwrap();
            assert_eq!(
                run(&mut database, &root).unwrap_err().to_string(),
                if backup_fails {
                    "SCHEMA_BACKUP_FAILED"
                } else {
                    "SCHEMA_MIGRATION_FAILED"
                }
            );
            if backup_fails {
                database.execute_batch("ROLLBACK").unwrap();
            }
            let marker = Handover::read(&root).unwrap().unwrap();
            assert_eq!(marker.phase, HandoverPhase::RecoveryRequired);
            assert_eq!(marker.migration.is_some(), !backup_fails);
            let files = std::fs::read_dir(&root).unwrap().count();
            for _ in 0..3 {
                assert_eq!(
                    run(&mut database, &root).unwrap_err().to_string(),
                    "SCHEMA_MIGRATION_FAILED"
                );
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), files);
                assert_eq!(Handover::read(&root).unwrap(), Some(marker.clone()));
            }
            if !backup_fails {
                let mut interrupted = marker.clone();
                interrupted.phase = HandoverPhase::Committed;
                interrupted.write(&root).unwrap();
                assert!(run(&mut database, &root).is_err());
                assert_eq!(Handover::read(&root).unwrap(), Some(marker));
                assert_eq!(std::fs::read_dir(&root).unwrap().count(), files);
            }
        }
    }

    #[test]
    fn backup_includes_committed_rows_still_in_the_source_wal() {
        let (_temp, root, memory) = fixture();
        drop(memory);
        let path = root.join("state.sqlite3");
        let _file = private::file(&path).unwrap();
        let mut database = Connection::open(&path).unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v1.sql"))
            .unwrap();
        database
            .execute_batch(
                "PRAGMA journal_mode=WAL; INSERT INTO legacy_probe VALUES (3, 'committed in WAL');",
            )
            .unwrap();
        assert!(
            std::fs::metadata(root.join("state.sqlite3-wal"))
                .unwrap()
                .len()
                > 0
        );
        run(&mut database, &root).unwrap();
        let name: String = database
            .query_row(
                "SELECT backup_file FROM migration_history WHERE to_version=2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let backup =
            Connection::open_with_flags(root.join(name), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            backup
                .query_row("SELECT value FROM legacy_probe WHERE id=3", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "committed in WAL"
        );
    }

    #[test]
    fn provisional_handover_cannot_change_schema_and_committed_marker_records_both_steps() {
        let (_temp, root, mut database) = fixture();
        let mut marker = Handover {
            old_hash: "a".repeat(64),
            target_hash: "b".repeat(64),
            phase: HandoverPhase::Starting,
            migration: None,
        };
        marker.write(&root).unwrap();
        assert_eq!(
            run(&mut database, &root).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        marker.phase = HandoverPhase::Committed;
        marker.write(&root).unwrap();
        run(&mut database, &root).unwrap();
        let completed = Handover::read(&root).unwrap().unwrap().migration.unwrap();
        assert!(completed.completed);
        let mut interrupted = Handover::read(&root).unwrap().unwrap();
        interrupted.migration.as_mut().unwrap().completed = false;
        interrupted.write(&root).unwrap();
        run(&mut database, &root).unwrap();
        assert_eq!(
            Handover::read(&root).unwrap().unwrap().migration,
            Some(completed)
        );
    }
}
