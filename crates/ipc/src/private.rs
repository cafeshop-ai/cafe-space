//! Owner-only state paths. Existing insecure paths are rejected, never repaired silently.

use std::{fs::File, io, path::Path};

/// The returned directory handle must stay alive while the directory is in use.
/// The parent must exist; this never changes permissions on ancestors.
pub fn directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory must be absolute",
        ));
    }
    #[cfg(windows)]
    {
        crate::windows_security::directory(path)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        check_unix(&file, true)?;
        Ok(file)
    }
}

/// Open or create a private regular file, without truncation or following a final symlink.
/// Caller keeps the private parent directory handle alive.
pub fn file(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        crate::windows_security::file(path)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        check_unix(&file, false)?;
        Ok(file)
    }
}

#[cfg(unix)]
fn check_unix(file: &File, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    #[allow(unsafe_code)]
    // SAFETY: geteuid has no arguments and returns the current process's effective UID.
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state path must be owned by this user and private",
        ));
    }
    #[cfg(target_os = "macos")]
    crate::macos_security::require_no_acl(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn private_paths_reopen_without_losing_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");
        let directory = directory(&path).unwrap();
        assert!(directory.metadata().unwrap().is_dir());
        let path = path.join("state");
        let mut first = file(&path).unwrap();
        first.write_all(b"preserved").unwrap();
        first.sync_all().unwrap();
        let mut reopened = file(&path).unwrap();
        let mut bytes = Vec::new();
        reopened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"preserved");
    }

    #[test]
    fn refuses_relative_directories_and_wrong_path_kinds() {
        assert!(directory(Path::new("relative")).is_err());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _dir = directory(&root).unwrap();
        let path = root.join("file");
        let _file = file(&path).unwrap();
        assert!(directory(&path).is_err());
        assert!(file(&root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_hardlinks_and_public_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _dir = directory(&root).unwrap();
        let target = root.join("target");
        let _file = file(&target).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(file(&link).is_err());
        let dir_link = temp.path().join("directory-link");
        symlink(&root, &dir_link).unwrap();
        assert!(directory(&dir_link).is_err());
        std::fs::hard_link(&target, root.join("hardlink")).unwrap();
        assert!(file(&target).is_err());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(directory(&root).is_err());
    }
}
