//! Explicit, per-user login registration. Never called by daemon startup or tests by default.

mod templates;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use crate::{BuildFlavor, private};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Install,
    Uninstall,
    Start,
    Stop,
    Verify,
}

/// Holds the same cross-process lock across staging, OS control and binary handover.
pub struct Lease {
    flavor: BuildFlavor,
    root: PathBuf,
    program: PathBuf,
    _root_guard: File,
    _lock: File,
}

impl Lease {
    pub fn acquire(flavor: BuildFlavor) -> io::Result<Self> {
        let root = flavor.data_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "user data directory unavailable")
        })?;
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let root_guard = private::directory(&root)?;
        let lock = lock(&root)?;
        let program = flavor.installed_daemon().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "installed daemon path unavailable")
        })?;
        Ok(Self {
            flavor,
            root,
            program,
            _root_guard: root_guard,
            _lock: lock,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Verify never starts, stops, registers, removes or overwrites a service.
    pub fn execute(&self, action: Action) -> io::Result<()> {
        let _program_guards = if matches!(action, Action::Install | Action::Start) {
            Some(check_program(&self.program)?)
        } else {
            None
        };
        #[cfg(unix)]
        {
            unix::execute(self.flavor, &self.program, action)
        }
        #[cfg(windows)]
        {
            windows::execute(self.flavor, &self.root, &self.program, action)
        }
    }
}

#[cfg(any(windows, test))]
fn check_removable_binary(program: &Path) -> io::Result<Option<File>> {
    let parent = program
        .parent()
        .ok_or_else(|| io::Error::other("missing bin directory"))?;
    let directory = match private::existing_directory(parent) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match private::existing_file(program) {
        Ok(file) => {
            drop(file);
            Ok(Some(directory))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(any(windows, test))]
fn remove_daemon_binary(program: &Path) -> io::Result<()> {
    if let Some(_directory) = check_removable_binary(program)? {
        std::fs::remove_file(program)?;
    }
    Ok(())
}

/// Only explicit calls may change the user's login startup configuration.
pub fn execute(flavor: BuildFlavor, action: Action) -> io::Result<()> {
    Lease::acquire(flavor)?.execute(action)
}

/// Inspect registration without creating a startup lock or contending with a handover.
pub fn verify(flavor: BuildFlavor) -> io::Result<()> {
    let root = flavor.data_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "user data directory unavailable")
    })?;
    std::fs::symlink_metadata(&root)?;
    let _guard = private::directory(&root)?;
    let program = flavor.installed_daemon().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "installed daemon path unavailable")
    })?;
    #[cfg(unix)]
    {
        unix::execute(flavor, &program, Action::Verify)
    }
    #[cfg(windows)]
    {
        windows::execute(flavor, &root, &program, Action::Verify)
    }
}

fn lock(root: &Path) -> io::Result<File> {
    let lock = private::file(&root.join("startup.lock"))?;
    // A concurrent fork can retain a dropped owner's lock until exec closes its copied fd.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another startup operation is running",
                ));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

fn check_program(program: &Path) -> io::Result<File> {
    templates::path_text(program)?;
    let directory =
        private::directory(program.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "missing bin directory")
        })?)?;
    let metadata = std::fs::symlink_metadata(program).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::new(
                io::ErrorKind::NotFound,
                "daemon must be installed in the private data directory's bin directory first",
            )
        } else {
            error
        }
    })?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "installed daemon must be a nonempty regular file",
        ));
    }
    let file = private::existing_file(program)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata()?.permissions().mode() & 0o100 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "installed daemon is not executable",
            ));
        }
    }
    drop(file);
    Ok(directory)
}

/// No shell, no captured user paths/output, no retries after an unknown outcome.
fn checked(command: &mut Command) -> io::Result<()> {
    let status = bounded_status(command)?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "startup operation failed (exit {:?}); inspect the OS service manager",
            status.code()
        )));
    }
    Ok(())
}

fn bounded_status(command: &mut Command) -> io::Result<std::process::ExitStatus> {
    bounded_status_until(command, Instant::now() + Duration::from_secs(30))
}

fn bounded_status_until(
    command: &mut Command,
    deadline: Instant,
) -> io::Result<std::process::ExitStatus> {
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "startup deadline expired",
        ));
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "startup command timed out; outcome unknown, inspect before retrying",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(any(target_os = "macos", test))]
fn launchd_loaded(code: Option<i32>) -> io::Result<bool> {
    match code {
        Some(0) => Ok(true),
        Some(113) => Ok(false), // launchctl ENOSERVICE, not a generic query failure.
        _ => Err(io::Error::other(
            "launchd state unknown; inspect the OS service manager",
        )),
    }
}

#[cfg(any(target_os = "macos", test))]
fn wait_for_launchd_unload(
    deadline: Instant,
    mut probe: impl FnMut(Instant) -> io::Result<Option<i32>>,
) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "launchd unload not confirmed; inspect before retrying",
            ));
        }
        if !launchd_loaded(probe(deadline)?)? {
            return Ok(());
        }
        std::thread::sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

#[cfg(windows)]
pub fn run_key(flavor: BuildFlavor, install: bool) -> io::Result<()> {
    let root = flavor.data_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "user data directory unavailable")
    })?;
    let _root_guard = private::directory(&root)?;
    let _lock = lock(&root)?;
    let program = flavor.installed_daemon().unwrap();
    let _guards = if install {
        Some(check_program(&program)?)
    } else {
        None
    };
    windows::run_key(flavor, &program, install)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_unload_waits_for_absence_and_never_guesses_after_unknown_queries() {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut codes = [Some(0), Some(0), Some(113)].into_iter();
        wait_for_launchd_unload(deadline, |budget| {
            assert_eq!(budget, deadline);
            Ok(codes.next().unwrap())
        })
        .unwrap();
        assert!(codes.next().is_none());
        for code in [Some(1), Some(13), Some(125), None] {
            let mut calls = 0;
            assert!(
                wait_for_launchd_unload(Instant::now() + Duration::from_secs(1), |_| {
                    calls += 1;
                    Ok(code)
                })
                .is_err()
            );
            assert_eq!(calls, 1);
        }
        let error = wait_for_launchd_unload(Instant::now() + Duration::from_secs(1), |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "probe unavailable",
            ))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let error =
            wait_for_launchd_unload(Instant::now(), |_| panic!("expired budget must not query"))
                .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let error =
            wait_for_launchd_unload(Instant::now() + Duration::from_millis(10), |_| Ok(Some(0)))
                .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let error = bounded_status_until(&mut Command::new("must-not-be-executed"), Instant::now())
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn uninstall_removes_only_the_daemon_and_preserves_recovery_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _root = private::directory(&root).unwrap();
        let program = root.join("bin/cafe-spaced.exe");
        remove_daemon_binary(&program).unwrap(); // Never installed.
        let _bin = private::directory(program.parent().unwrap()).unwrap();
        for file in ["state.sqlite3", "recovery.json", "bin/cafe-spaced.exe.prev"] {
            let path = root.join(file);
            drop(private::file(&path).unwrap());
            std::fs::write(path, b"preserved").unwrap();
        }
        drop(private::file(&program).unwrap());
        std::fs::write(&program, b"daemon").unwrap();
        remove_daemon_binary(&program).unwrap();
        remove_daemon_binary(&program).unwrap(); // Idempotent.
        assert!(!program.exists());
        for file in ["state.sqlite3", "recovery.json", "bin/cafe-spaced.exe.prev"] {
            assert_eq!(std::fs::read(root.join(file)).unwrap(), b"preserved");
        }
        std::fs::create_dir(&program).unwrap();
        assert!(remove_daemon_binary(&program).is_err());
        assert!(program.is_dir());
    }

    #[test]
    fn startup_lock_tolerates_brief_contention_without_stealing_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _guard = private::directory(&root).unwrap();
        let first = lock(&root).unwrap();
        #[cfg(unix)]
        let first = {
            let copied = first.try_clone().unwrap();
            drop(first);
            copied // Models the open-file-description copy retained between fork and exec.
        };
        let probe = private::file(&root.join("startup.lock")).unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(probe);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(first);
        });
        let next = lock(&root).unwrap();
        release.join().unwrap();
        drop(next);
    }

    #[test]
    fn startup_ownership_is_exclusive_without_registering_anything() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _guard = private::directory(&root).unwrap();
        let first = lock(&root).unwrap();
        assert_eq!(lock(&root).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        drop(first);
        drop(lock(&root).unwrap());
        assert!(check_program(&root.join("bin/missing")).is_err());
    }
}
