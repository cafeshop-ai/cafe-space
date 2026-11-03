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
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Install,
    Uninstall,
    Start,
    Stop,
}

/// Only these explicit calls may change the user's login startup configuration.
pub fn execute(flavor: BuildFlavor, action: Action) -> io::Result<()> {
    let root = flavor.data_dir().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "user data directory unavailable")
    })?;
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _root_guard = private::directory(&root)?;
    let _lock = lock(&root)?;
    let program = flavor.installed_daemon().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "installed daemon path unavailable")
    })?;
    let _program_guards = if matches!(action, Action::Install | Action::Start) {
        Some(check_program(&program)?)
    } else {
        None
    };
    #[cfg(unix)]
    {
        unix::execute(flavor, &program, action)
    }
    #[cfg(windows)]
    {
        windows::execute(flavor, &root, &program, action)
    }
}

fn lock(root: &Path) -> io::Result<File> {
    let lock = private::file(&root.join("startup.lock"))?;
    lock.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "another startup operation is running",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(lock)
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
    let file = private::file(program)?;
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
    // Writable executable handles prevent exec on Unix and Windows; close before launching.
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
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
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
