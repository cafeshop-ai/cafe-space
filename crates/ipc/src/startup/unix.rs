#[cfg(target_os = "macos")]
use super::bounded_status;
use super::{Action, checked, templates};
use crate::{BuildFlavor, private};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

fn config_path(flavor: BuildFlavor) -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join(".config"))
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "user config directory unavailable")
            })?;
        Ok(config
            .join("systemd/user")
            .join(format!("{}.service", flavor.daemon_name())))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "user home directory unavailable")
            })?;
        Ok(home
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", flavor.service_label())))
    }
}

fn definition(flavor: BuildFlavor, program: &Path) -> io::Result<String> {
    #[cfg(target_os = "linux")]
    {
        templates::systemd(flavor, program)
    }
    #[cfg(target_os = "macos")]
    {
        templates::launchd(flavor, program)
    }
}

/// Refuse to adopt another program's configuration, even if its label collides.
fn verify_file(path: &Path, expected: &str) -> io::Result<File> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "startup definition must be a regular file",
        ));
    }
    let mut file = private::existing_file(path)?;
    let mut content = String::new();
    std::io::Read::by_ref(&mut file)
        .take(64 * 1024)
        .read_to_string(&mut content)?;
    if content != expected {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "startup definition differs; refusing to overwrite or remove it",
        ));
    }
    Ok(file)
}

fn config_parent(path: &Path) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing startup parent"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)?;
    let metadata = directory.metadata()?;
    #[allow(unsafe_code)]
    // SAFETY: geteuid has no arguments and returns this process's effective UID.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "startup config directory must be owner-controlled",
        ));
    }
    #[cfg(target_os = "macos")]
    crate::macos_security::require_no_acl(&directory)?;
    Ok(directory)
}

pub(super) fn execute(flavor: BuildFlavor, program: &Path, action: Action) -> io::Result<()> {
    let path = config_path(flavor)?;
    let expected = definition(flavor, program)?;
    if action == Action::Verify {
        std::fs::symlink_metadata(&path)?;
    }
    let _config_directory = config_parent(&path)?;
    if action == Action::Install {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        file.write_all(expected.as_bytes())?;
        file.sync_all()?;
        File::open(path.parent().unwrap())?.sync_all()?;
        #[cfg(target_os = "linux")]
        {
            checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
            checked(
                Command::new("systemctl")
                    .args(["--user", "enable"])
                    .arg(&path),
            )?;
        }
        // launchd reads ~/Library/LaunchAgents at the next login; installation never starts it now.
        return Ok(());
    }
    let _definition_guard = verify_file(&path, &expected)?;
    if action == Action::Verify {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let unit = format!("{}.service", flavor.daemon_name());
        match action {
            Action::Start => checked(Command::new("systemctl").args(["--user", "start", &unit])),
            Action::Stop => checked(Command::new("systemctl").args(["--user", "stop", &unit])),
            Action::Uninstall => {
                checked(Command::new("systemctl").args(["--user", "disable", "--now", &unit]))?;
                std::fs::remove_file(&path)?;
                checked(Command::new("systemctl").args(["--user", "daemon-reload"]))
            }
            Action::Install | Action::Verify => unreachable!(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        #[allow(unsafe_code)]
        // SAFETY: geteuid has no arguments and returns the current effective UID.
        let uid = unsafe { libc::geteuid() };
        let domain = format!("gui/{uid}");
        let target = format!("{domain}/{}", flavor.service_label());
        let loaded = bounded_status(Command::new("launchctl").args(["print", &target]))?.success();
        match action {
            Action::Start if loaded => {
                checked(Command::new("launchctl").args(["kickstart", &target]))
            }
            Action::Start => checked(
                Command::new("launchctl")
                    .args(["bootstrap", &domain])
                    .arg(&path),
            ),
            Action::Stop | Action::Uninstall => {
                if loaded {
                    checked(Command::new("launchctl").args(["bootout", &target]))?;
                }
                if action == Action::Uninstall {
                    std::fs::remove_file(&path)?;
                }
                Ok(())
            }
            Action::Install | Action::Verify => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_definition_passes_os_syntax_check_without_installing() {
        let temp = tempfile::tempdir().unwrap();
        let program = std::env::current_exe().unwrap();
        let text = definition(BuildFlavor::Dev, &program).unwrap();
        #[cfg(target_os = "linux")]
        let path = temp.path().join("cafe-spaced-dev.service");
        #[cfg(target_os = "macos")]
        let path = temp.path().join("ai.cafeshop.cafe-spaced-dev.plist");
        std::fs::write(&path, text).unwrap();
        #[cfg(target_os = "linux")]
        let status = Command::new("systemd-analyze")
            .arg("verify")
            .arg(&path)
            .status()
            .unwrap();
        #[cfg(target_os = "macos")]
        let status = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn config_directory_may_be_readable_but_not_writable_by_others() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config/startup");
        drop(config_parent(&path).unwrap());
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        drop(config_parent(&path).unwrap());
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(config_parent(&path).is_err());
        let link = temp.path().join("link");
        symlink(path.parent().unwrap(), &link).unwrap();
        assert!(config_parent(&link.join("startup")).is_err());
    }

    #[test]
    fn refuses_unrelated_definition_without_registering_a_service() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("startup");
        let mut handle = private::file(&file).unwrap();
        handle.write_all(b"unrelated").unwrap();
        assert!(verify_file(&file, "managed").is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "unrelated");
    }
}
