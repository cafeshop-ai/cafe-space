//! Private on-disk state, versioned schema and single-instance ownership.

use cafe_space_ipc::{BuildFlavor, private};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{File, TryLockError},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const APPLICATION_ID: i32 = 0x43414645;

pub struct State {
    // Drop SQLite before releasing the file/lock/directory guards.
    pub(crate) database: Connection,
    _database_file: File,
    log: File,
    log_path: PathBuf,
    log_failures: u64,
    _logs_directory: File,
    identity: File,
    _lock: File,
    _root: File,
}

impl State {
    pub fn open(root: &Path, flavor: BuildFlavor) -> io::Result<Self> {
        validate_root(root, flavor)?;
        let root_guard = private::directory(root)?;
        let mut lock = private::file(&root.join(format!("{}.lock", flavor.daemon_name())))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "daemon is already running for this data directory",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        // Never unlink the lock: waiters must continue to refer to the same file.
        lock.set_len(0)?;
        writeln!(lock, "{}", std::process::id())?;
        lock.sync_all()?;

        let database_path = root.join("state.sqlite3");
        let database_file = private::file(&database_path)?;
        // Explicit owners matter on elevated Windows tokens: inherited ACLs alone can leave an Administrators owner.
        for suffix in ["-wal", "-shm", "-journal"] {
            drop(private::file(&root.join(format!("state.sqlite3{suffix}")))?);
        }
        let mut database = Connection::open_with_flags(
            &database_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(io::Error::other)?;
        database
            .busy_timeout(Duration::from_secs(5))
            .map_err(io::Error::other)?;
        let app_id: i32 = database
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        let version: i32 = database
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        if app_id != 0 && app_id != APPLICATION_ID
            || !(0..=crate::migrations::SCHEMA_VERSION).contains(&version)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported state database; refusing to migrate",
            ));
        }
        if app_id == 0 {
            let tables: i64 = database
                .query_row(
                    "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            if tables != 0 || version != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "state path contains an unrecognized database",
                ));
            }
        }
        let uncommitted =
            cafe_space_ipc::installation::Handover::read(root)?.is_some_and(|marker| {
                !matches!(
                    marker.phase,
                    cafe_space_ipc::installation::HandoverPhase::Committed
                        | cafe_space_ipc::installation::HandoverPhase::RolledBack
                        | cafe_space_ipc::installation::HandoverPhase::NotApplied
                )
            });
        if version == 0 && uncommitted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cannot initialize a schema during an uncommitted handover",
            ));
        }
        let journal: String = database
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        if journal != "wal" {
            return Err(io::Error::other("SQLite WAL mode unavailable"));
        }
        database
            .execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
            .map_err(io::Error::other)?;
        if version == 0 {
            let transaction = database.transaction().map_err(io::Error::other)?;
            transaction
                .pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(io::Error::other)?;
            transaction
                .pragma_update(None, "user_version", 1)
                .map_err(io::Error::other)?;
            transaction.commit().map_err(io::Error::other)?;
        }

        let logs_directory = private::directory(&root.join("logs"))?;
        let log = private::file(&root.join("logs/daemon.log"))?;
        let mut state = Self {
            database,
            _database_file: database_file,
            log,
            log_path: root.join("logs/daemon.log"),
            log_failures: 0,
            _logs_directory: logs_directory,
            identity: private::file(&root.join("daemon.instance"))?,
            _lock: lock,
            _root: root_guard,
        };
        if !uncommitted {
            crate::migrations::run(&mut state.database, root)?;
        }
        state.log("DAEMON_STATE_READY")?;
        Ok(state)
    }

    pub fn mark_boot(&mut self, stream_id: &str) -> io::Result<()> {
        if stream_id.len() != 32
            || !stream_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid daemon boot ID",
            ));
        }
        self.identity.rewind()?;
        self.identity.set_len(0)?;
        writeln!(self.identity, "{} {stream_id}", std::process::id())?;
        self.identity.sync_all()
    }

    /// The old binary's rollback check: no schema creation, log writes, migrations or DB replacement.
    pub fn check_compatible(root: &Path, flavor: BuildFlavor) -> io::Result<()> {
        validate_root(root, flavor)?;
        std::fs::symlink_metadata(root)?;
        let _root = private::directory(root)?;
        let lock = private::existing_file(&root.join(format!("{}.lock", flavor.daemon_name())))?;
        lock.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => {
                io::Error::new(io::ErrorKind::WouldBlock, "daemon is still running")
            }
            TryLockError::Error(error) => error,
        })?;
        let path = root.join("state.sqlite3");
        let _database = private::existing_file(&path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            match private::existing_file(&root.join(format!("state.sqlite3{suffix}"))) {
                Ok(file) => {
                    if suffix != "-shm" && file.metadata()?.len() != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "state has uncheckpointed recovery data",
                        ));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        // READ_ONLY alone creates WAL/SHM; immutable is safe only after the recovery-data check above.
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
        let app_id: i32 = database
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        let version: i32 = database
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        let integrity: String = database
            .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
            .map_err(io::Error::other)?;
        if app_id != APPLICATION_ID
            || !(1..=crate::migrations::SCHEMA_VERSION).contains(&version)
            || integrity != "ok"
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "state is not compatible; binary rollback is refused",
            ));
        }
        Ok(())
    }

    pub fn checkpoint(&self) -> io::Result<()> {
        let (busy, frames, checkpointed): (i32, i32, i32) = self
            .database
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(io::Error::other)?;
        if busy != 0 || frames != checkpointed {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "state checkpoint did not complete",
            ));
        }
        Ok(())
    }

    /// Only invalid codes return an error; diagnostic I/O failures are counted, not fatal.
    pub fn log(&mut self, code: &str) -> io::Result<()> {
        if code.is_empty()
            || code.len() > 64
            || !code.bytes().all(|c| c.is_ascii_uppercase() || c == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid diagnostic code",
            ));
        }
        if self.write_log(code).is_err() {
            self.log_failures = self.log_failures.saturating_add(1);
        }
        Ok(())
    }

    fn write_log(&mut self, code: &str) -> io::Result<()> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        let line = format!("{seconds} {code}\n");
        if self.log.metadata()?.len() + line.len() as u64 > MAX_LOG_BYTES {
            let temporary = self.log_path.with_extension("log.1.next");
            let mut previous = private::file(&temporary)?;
            previous.set_len(0)?;
            previous.rewind()?;
            self.log.rewind()?;
            io::copy(
                &mut Read::by_ref(&mut self.log).take(MAX_LOG_BYTES),
                &mut previous,
            )?;
            previous.sync_all()?;
            drop(previous);
            private::replace_durable(&temporary, &self.log_path.with_extension("log.1"))?;
            self.log.set_len(0)?;
        }
        self.log.seek(SeekFrom::End(0))?;
        self.log.write_all(line.as_bytes())?;
        self.log.sync_data()
    }
}

fn validate_root(root: &Path, flavor: BuildFlavor) -> io::Result<()> {
    if root.file_name() != Some(std::ffi::OsStr::new(flavor.directory_name())) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state directory does not match the executable flavor",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_rotation_retains_exactly_one_bounded_previous_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let mut state = State::open(&root, BuildFlavor::Dev).unwrap();
        state.log.set_len(MAX_LOG_BYTES).unwrap();
        let first = std::fs::read(&state.log_path).unwrap();
        state.log("ROTATE_FIRST").unwrap();
        let previous = root.join("logs/daemon.log.1");
        assert_eq!(std::fs::read(&previous).unwrap(), first);
        state.log.set_len(MAX_LOG_BYTES).unwrap();
        let second = std::fs::read(&state.log_path).unwrap();
        state.log("ROTATE_SECOND").unwrap();
        assert_eq!(std::fs::read(&previous).unwrap(), second);
        assert_eq!(std::fs::read_dir(root.join("logs")).unwrap().count(), 2);
        assert!(state.log.metadata().unwrap().len() <= MAX_LOG_BYTES);
        assert_eq!(state.log_failures, 0);
        drop(state);
        let _reopened = State::open(&root, BuildFlavor::Dev).unwrap();
        assert_eq!(std::fs::read(previous).unwrap(), second);
    }

    #[test]
    fn rotation_failure_preserves_current_log_and_does_not_stop_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let mut state = State::open(&root, BuildFlavor::Dev).unwrap();
        state.log.set_len(MAX_LOG_BYTES).unwrap();
        let before = std::fs::read(&state.log_path).unwrap();
        let _unexpected = private::directory(&root.join("logs/daemon.log.1")).unwrap();
        state.log("ROTATE_REFUSED").unwrap();
        assert_eq!(state.log_failures, 1);
        assert_eq!(std::fs::read(&state.log_path).unwrap(), before);
        assert_eq!(
            state
                .database
                .query_row("SELECT 1", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn diagnostic_write_failure_does_not_stop_state_or_expose_invalid_codes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let mut state = State::open(&root, BuildFlavor::Dev).unwrap();
        state.log = private::existing_file(&root.join("logs/daemon.log")).unwrap();
        state.log("IPC_ACCEPT_RETRY").unwrap();
        assert_eq!(state.log_failures, 1);
        assert!(state.log("private payload").is_err());
        assert_eq!(state.log_failures, 1);
        let value: i32 = state
            .database
            .query_row("SELECT 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
        assert!(
            !std::fs::read_to_string(root.join("logs/daemon.log"))
                .unwrap()
                .contains("private payload")
        );
    }

    #[test]
    fn boot_identity_is_readable_while_the_single_instance_lock_is_held() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let mut state = State::open(&root, BuildFlavor::Dev).unwrap();
        state.mark_boot(&"a".repeat(32)).unwrap();
        let identity = cafe_space_ipc::installation::daemon_identity(&root).unwrap();
        assert_eq!(identity.pid, std::process::id());
        assert_eq!(identity.event_stream_id, "a".repeat(32));
        assert!(state.mark_boot("invalid").is_err());
        assert_eq!(
            cafe_space_ipc::installation::daemon_identity(&root).unwrap(),
            identity
        );
    }

    #[test]
    fn rollback_check_is_read_only_and_refuses_live_or_newer_state() {
        let temp = tempfile::Builder::new()
            .prefix("café 中文 #% ")
            .tempdir()
            .unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        assert!(State::check_compatible(&root, BuildFlavor::Dev).is_err());
        drop(state);
        let before = std::fs::read(root.join("state.sqlite3")).unwrap();
        let sidecars = || {
            ["-wal", "-shm", "-journal"]
                .map(|suffix| std::fs::read(root.join(format!("state.sqlite3{suffix}"))).ok())
        };
        let previous_sidecars = sidecars();
        State::check_compatible(&root, BuildFlavor::Dev).unwrap();
        assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), before);
        assert!(
            sidecars() == previous_sidecars,
            "read-only checks must not create or change sidecars"
        );
        drop(State::open(&root, BuildFlavor::Dev).unwrap());
        let database = Connection::open(root.join("state.sqlite3")).unwrap();
        database.pragma_update(None, "user_version", 99).unwrap();
        drop(database);
        let incompatible = std::fs::read(root.join("state.sqlite3")).unwrap();
        assert!(State::check_compatible(&root, BuildFlavor::Dev).is_err());
        assert_eq!(
            std::fs::read(root.join("state.sqlite3")).unwrap(),
            incompatible
        );
    }

    #[test]
    fn rollback_check_never_ignores_uncheckpointed_wal_or_hot_journal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        drop(State::open(&root, BuildFlavor::Dev).unwrap());
        let before = std::fs::read(root.join("state.sqlite3")).unwrap();
        for suffix in ["-wal", "-journal"] {
            let path = root.join(format!("state.sqlite3{suffix}"));
            let mut file = private::file(&path).unwrap();
            file.write_all(b"UNCOMMITTED_RECOVERY_BYTES").unwrap();
            assert_eq!(
                State::check_compatible(&root, BuildFlavor::Dev)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"UNCOMMITTED_RECOVERY_BYTES");
            assert_eq!(std::fs::read(root.join("state.sqlite3")).unwrap(), before);
            file.set_len(0).unwrap();
        }
    }

    #[test]
    fn uncommitted_handover_cannot_initialize_or_migrate_a_schema() {
        use cafe_space_ipc::installation::{Handover, HandoverPhase};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        state
            .database
            .pragma_update(None, "user_version", 0)
            .unwrap();
        drop(state);
        Handover {
            old_hash: "a".repeat(64),
            target_hash: "b".repeat(64),
            phase: HandoverPhase::Starting,
            migration: None,
        }
        .write(&root)
        .unwrap();
        assert_eq!(
            State::open(&root, BuildFlavor::Dev).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        let database = Connection::open_with_flags(
            root.join("state.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let version: i32 = database
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn wal_and_version_persist_and_lock_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        assert!(State::open(&root, BuildFlavor::Release).is_err());
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        assert_eq!(
            State::open(&root, BuildFlavor::Dev).err().unwrap().kind(),
            io::ErrorKind::AlreadyExists
        );
        let mode: String = state
            .database
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        state
            .database
            .execute_batch(
                "CREATE TABLE probe (value TEXT); INSERT INTO probe VALUES ('preserved');",
            )
            .unwrap();
        drop(state);
        let mut reopened = State::open(&root, BuildFlavor::Dev).unwrap();
        let value: String = reopened
            .database
            .query_row("SELECT value FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "preserved");
        assert_eq!(
            reopened
                .database
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            crate::migrations::SCHEMA_VERSION
        );
        assert_eq!(
            reopened
                .database
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        reopened.log("DAEMON_STOPPED").unwrap();
        assert!(reopened.log("payload secret").is_err());
    }

    #[test]
    fn rejects_newer_or_foreign_database_without_migrating() {
        for (application, version) in [(APPLICATION_ID, 3), (APPLICATION_ID, -1), (123, 1), (0, 1)]
        {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(BuildFlavor::Dev.directory_name());
            let state = State::open(&root, BuildFlavor::Dev).unwrap();
            state
                .database
                .pragma_update(None, "application_id", application)
                .unwrap();
            state
                .database
                .pragma_update(None, "user_version", version)
                .unwrap();
            drop(state);
            assert!(State::open(&root, BuildFlavor::Dev).is_err());
            let connection = Connection::open(root.join("state.sqlite3")).unwrap();
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
                    .unwrap(),
                version
            );
        }
    }

    #[test]
    #[ignore = "worker spawned by killed_process_releases_lock_and_recovers_wal"]
    fn crash_worker() {
        let root = std::path::PathBuf::from(std::env::var_os("CAFE_SPACE_TEST_STATE").unwrap());
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        let starting = crate::sessions::tests::approved(&state.database, "crash-session");
        let mut ready = starting.clone();
        ready.state = cafe_space_protocol::SessionState::Ready;
        let ready = crate::sessions::transition(&state.database, &starting, ready)
            .unwrap()
            .0;
        let mut running = ready.clone();
        running.state = cafe_space_protocol::SessionState::Running;
        running.active_run_id = Some("crash-run".into());
        crate::sessions::transition(&state.database, &ready, running).unwrap();
        state.database.execute_batch("CREATE TABLE probe (value TEXT); INSERT INTO probe VALUES ('committed'); BEGIN IMMEDIATE; INSERT INTO probe VALUES ('uncommitted');").unwrap();
        for suffix in ["-wal", "-shm"] {
            let path = root.join(format!("state.sqlite3{suffix}"));
            assert!(path.exists());
            drop(private::file(&path).unwrap());
        }
        std::fs::write(root.join("worker-ready"), b"ready").unwrap();
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn killed_process_releases_lock_and_recovers_wal() {
        struct Worker(std::process::Child);
        impl Drop for Worker {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let mut worker = Worker(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "state::tests::crash_worker",
                    "--nocapture",
                ])
                .env("CAFE_SPACE_TEST_STATE", &root)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !root.join("worker-ready").exists() {
            assert!(
                worker.0.try_wait().unwrap().is_none(),
                "worker exited before opening state"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "worker readiness timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            State::open(&root, BuildFlavor::Dev).err().unwrap().kind(),
            io::ErrorKind::AlreadyExists
        );
        worker.0.kill().unwrap();
        worker.0.wait().unwrap();
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        let values = state
            .database
            .prepare("SELECT value FROM probe")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(values, ["committed"]);
        let before_recovery = crate::sessions::get(&state.database, "crash-session")
            .unwrap()
            .unwrap();
        assert_eq!(
            before_recovery.state,
            cafe_space_protocol::SessionState::Running
        );
        assert_eq!(
            state
                .database
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        let host =
            crate::host::Host::new(state, &root, "restarted-boot".into(), "a".repeat(64)).unwrap();
        let response = host
            .dispatch(
                crate::update::Access::Local,
                1,
                cafe_space_protocol::Request::GetSession(cafe_space_protocol::GetSessionParams {
                    session_id: "crash-session".into(),
                }),
            )
            .unwrap();
        let cafe_space_protocol::RequestResult::Applied(data) = response.result else {
            panic!("recovered session")
        };
        let cafe_space_protocol::Response::Session(session) = data.result else {
            panic!("recovered session")
        };
        assert_eq!(
            session.session.session.state,
            cafe_space_protocol::SessionState::Stopped
        );
        assert_eq!(
            session.session.session.stop_reason,
            Some(cafe_space_protocol::SessionStopReason::DaemonRestarted)
        );
        assert_eq!(session.session.session.active_run_id, None);
        assert_eq!(session.session.session.stream_id, before_recovery.stream_id);
        assert!(session.session.messages.is_empty());
    }

    #[test]
    fn log_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::open(
            &temp.path().join(BuildFlavor::Dev.directory_name()),
            BuildFlavor::Dev,
        )
        .unwrap();
        state.log.set_len(MAX_LOG_BYTES).unwrap();
        state.log("LIMIT_REACHED").unwrap();
        assert!(state.log.metadata().unwrap().len() < 128);
    }
}
