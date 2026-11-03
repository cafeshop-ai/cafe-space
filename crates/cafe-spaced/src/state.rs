//! Private on-disk state and single-instance ownership. No session data is created in M0.

use cafe_space_ipc::{BuildFlavor, private};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::{File, TryLockError},
    io::{self, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const APPLICATION_ID: i32 = 0x43414645;

pub struct State {
    // Drop SQLite before releasing the file/lock/directory guards.
    pub database: Connection,
    _database_file: File,
    log: File,
    _logs_directory: File,
    _lock: File,
    _root: File,
}

impl State {
    pub fn open(root: &Path, flavor: BuildFlavor) -> io::Result<Self> {
        if root.file_name() != Some(std::ffi::OsStr::new(flavor.directory_name())) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "state directory does not match the executable flavor",
            ));
        }
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
        if app_id != 0 && app_id != APPLICATION_ID || !(0..=1).contains(&version) {
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
            _logs_directory: logs_directory,
            _lock: lock,
            _root: root_guard,
        };
        state.log("DAEMON_STATE_READY")?;
        Ok(state)
    }

    /// Only bounded diagnostic codes, never request payloads, credentials or user paths.
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
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        let line = format!("{seconds} {code}\n");
        // shortcut: M0 keeps one bounded diagnostic log; add retention when production operations need it.
        if self.log.metadata()?.len() + line.len() as u64 > MAX_LOG_BYTES {
            self.log.set_len(0)?;
        }
        self.log.seek(SeekFrom::End(0))?;
        self.log.write_all(line.as_bytes())?;
        self.log.sync_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            1
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
        for (application, version) in [(APPLICATION_ID, 2), (APPLICATION_ID, -1), (123, 1), (0, 1)]
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
        assert_eq!(
            state
                .database
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
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
