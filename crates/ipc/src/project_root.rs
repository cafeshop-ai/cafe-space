//! Native directory identity for approved cwd roots; not an agent sandbox.
#![allow(unsafe_code)]

use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory {
    pub path: PathBuf,
    pub identity: String,
}

fn rejected(code: &'static str) -> io::Error {
    io::Error::other(code)
}

/// Resolve existing local directories; callers recheck this identity immediately before spawn.
pub fn inspect(path: &Path) -> io::Result<Directory> {
    let text = path
        .to_str()
        .ok_or_else(|| rejected("PROJECT_ROOT_NOT_UTF8"))?;
    if !path.is_absolute() || text.chars().any(char::is_control) || unsupported_windows_path(text) {
        return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| rejected("PROJECT_ROOT_UNAVAILABLE"))?;
    let canonical_text = canonical
        .to_str()
        .ok_or_else(|| rejected("PROJECT_ROOT_NOT_UTF8"))?;
    let canonical = normalize_windows_prefix(canonical_text)?;
    if canonical.parent().is_none() {
        return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
    }
    let file = open(&canonical)?;
    if !file
        .metadata()
        .map_err(|_| rejected("PROJECT_ROOT_UNAVAILABLE"))?
        .is_dir()
    {
        return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
    }
    reject_network(&file, &canonical)?;
    Ok(Directory {
        path: canonical,
        identity: identity(&file)?,
    })
}

/// Identity comparison also covers case aliases on case-insensitive filesystems.
pub fn is_within(directory: &Directory, ancestor: &Path) -> io::Result<bool> {
    let ancestor = identity(&open(ancestor)?)?;
    for path in directory.path.ancestors() {
        if identity(&open(path)?)? == ancestor {
            return Ok(true);
        }
    }
    Ok(false)
}

fn unsupported_windows_path(text: &str) -> bool {
    #[cfg(windows)]
    {
        let lower = text.replace('/', "\\").to_ascii_lowercase();
        lower.starts_with("\\\\?\\unc\\")
            || lower.starts_with("\\\\.\\")
            || (lower.starts_with("\\\\") && !lower.starts_with("\\\\?\\"))
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        false
    }
}

fn normalize_windows_prefix(text: &str) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        if unsupported_windows_path(text) {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
        Ok(PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(text)))
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(text))
    }
}

fn open(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS);
    }
    options
        .open(path)
        .map_err(|_| rejected("PROJECT_ROOT_UNAVAILABLE"))
}

fn identity(file: &File) -> io::Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file
            .metadata()
            .map_err(|_| rejected("PROJECT_ROOT_UNAVAILABLE"))?;
        let volume = volume_id(file)?;
        Ok(unix_identity(&volume, meta.dev(), meta.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };
        let mut info = std::mem::MaybeUninit::<FILE_ID_INFO>::zeroed();
        // SAFETY: the borrowed handle is live and info is a correctly sized writable FILE_ID_INFO.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
        }
        // SAFETY: successful FileIdInfo initialized every field above.
        let info = unsafe { info.assume_init() };
        let id: String = info
            .FileId
            .Identifier
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(format!("{:016x}:{id}", info.VolumeSerialNumber))
    }
}

#[cfg(any(unix, test))]
fn unix_identity(volume: &str, _device: u64, inode: u64) -> String {
    format!("{volume}:{inode:016x}")
}

#[cfg(target_os = "linux")]
fn volume_id(file: &File) -> io::Result<String> {
    use std::os::fd::AsRawFd;
    let mut info = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: fd is live and info points to writable statfs storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
    }
    // SAFETY: fstatfs initialized info; Linux fsid_t is two contiguous c_int values.
    let info = unsafe { info.assume_init() };
    assert_eq!(
        std::mem::size_of::<libc::fsid_t>(),
        std::mem::size_of::<[i32; 2]>()
    );
    // SAFETY: checked equal size; read_unaligned does not require stronger alignment than fsid_t.
    let id = unsafe {
        std::ptr::read_unaligned((&info.f_fsid as *const libc::fsid_t).cast::<[i32; 2]>())
    };
    if id == [0, 0] {
        return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
    }
    Ok(format!("{:08x}{:08x}", id[0] as u32, id[1] as u32))
}

#[cfg(target_os = "macos")]
fn volume_id(file: &File) -> io::Result<String> {
    use std::os::fd::AsRawFd;
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_UUID | libc::ATTR_VOL_INFO,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut output = [0u8; 20];
    // SAFETY: fd is live; attributes and output are writable, sized buffers. UUID follows a u32 length.
    if unsafe {
        libc::fgetattrlist(
            file.as_raw_fd(),
            (&mut attributes as *mut libc::attrlist).cast(),
            output.as_mut_ptr().cast(),
            output.len(),
            0,
        )
    } != 0
    {
        return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
    }
    if u32::from_ne_bytes(output[..4].try_into().unwrap()) != 20
        || output[4..].iter().all(|b| *b == 0)
    {
        return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
    }
    Ok(output[4..]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn reject_network(file: &File, path: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let mut info = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        // SAFETY: the borrowed directory fd is live and info is writable statfs storage.
        if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } != 0 {
            return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
        }
        // SAFETY: fstatfs succeeded above.
        let kind = unsafe { info.assume_init() }.f_type as u64;
        if linux_network(kind) {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
        // Mount IDs distinguish bind mounts even when parent and child share st_dev.
        if mount_id(file)?
            != mount_id(&open(
                path.parent()
                    .ok_or_else(|| rejected("PROJECT_ROOT_UNSUPPORTED"))?,
            )?)?
        {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let mut info = std::mem::MaybeUninit::<libc::statfs>::zeroed();
        // SAFETY: the borrowed directory fd is live and info is writable statfs storage.
        if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } != 0 {
            return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
        }
        // SAFETY: fstatfs succeeded above; MNT_LOCAL excludes NFS/SMB and other remote mounts.
        let info = unsafe { info.assume_init() };
        if info.f_flags & libc::MNT_LOCAL == 0 {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
        // SAFETY: the kernel initializes f_mntonname as a NUL-terminated path inside statfs.
        let mount = unsafe { std::ffi::CStr::from_ptr(info.f_mntonname.as_ptr()) }
            .to_str()
            .map_err(|_| rejected("PROJECT_ROOT_NOT_UTF8"))?;
        if identity(file)? == identity(&open(Path::new(mount))?)? {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
        let _ = path;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
        let text: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut volume = vec![0u16; 32768];
        // SAFETY: text is NUL-terminated and volume is a writable buffer of the supplied length.
        if unsafe { GetVolumePathNameW(text.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) }
            == 0
        {
            return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
        }
        // SAFETY: GetVolumePathNameW wrote a terminated volume path; no buffer is moved here.
        let kind = unsafe { GetDriveTypeW(volume.as_ptr()) };
        if kind == 0 || kind == 1 || kind == 4 {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
        let end = volume.iter().position(|value| *value == 0).unwrap();
        let volume = PathBuf::from(
            String::from_utf16(&volume[..end]).map_err(|_| rejected("PROJECT_ROOT_NOT_UTF8"))?,
        );
        if identity(file)? == identity(&open(&volume)?)? {
            return Err(rejected("PROJECT_ROOT_UNSUPPORTED"));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn mount_id(file: &File) -> io::Result<u64> {
    use std::os::fd::AsRawFd;
    let mut info = std::mem::MaybeUninit::<libc::statx>::zeroed();
    // SAFETY: the borrowed fd and empty C string are live; statx writes the sized buffer.
    if unsafe {
        libc::statx(
            file.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_MNT_ID,
            info.as_mut_ptr(),
        )
    } != 0
    {
        return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
    }
    // SAFETY: statx succeeded; check its mask before using the optional mount ID.
    let info = unsafe { info.assume_init() };
    if info.stx_mask & libc::STATX_MNT_ID == 0 {
        return Err(rejected("PROJECT_ROOT_IDENTITY_UNAVAILABLE"));
    }
    Ok(info.stx_mnt_id)
}

#[cfg(any(target_os = "linux", test))]
fn linux_network(kind: u64) -> bool {
    matches!(
        kind,
        0x6969 | 0x517b | 0xff534d42 | 0xfe534d42 | 0x564c | 0x73757245
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires tmpfs and bind mounts in a disposable runner's private mount namespace"]
    fn mounted_volume_and_bind_roots_are_rejected_but_children_are_allowed() {
        assert_eq!(
            std::env::var("RUNNER_ENVIRONMENT").as_deref(),
            Ok("github-hosted")
        );
        let base = PathBuf::from(
            std::env::var_os("CAFE_SPACE_TEST_MOUNTS").expect("isolated mounts required"),
        );
        for name in ["volume", "bind"] {
            assert_eq!(
                inspect(&base.join(name)).unwrap_err().to_string(),
                "PROJECT_ROOT_UNSUPPORTED"
            );
            inspect(&base.join(name).join("child")).unwrap();
        }
        assert_eq!(
            inspect(&base.join("local")).unwrap().identity,
            identity(&open(&base.join("bind")).unwrap()).unwrap()
        );
        println!(
            "M1_PROJECT_MOUNT_ROOT_OK tmpfsRejected=true bindRejected=true childrenAccepted=true"
        );
    }

    #[test]
    fn unix_volume_identity_does_not_depend_on_device_number() {
        assert_eq!(
            unix_identity("volume-uuid", 1, 42),
            unix_identity("volume-uuid", 999, 42)
        );
        assert_ne!(
            unix_identity("volume-uuid", 1, 42),
            unix_identity("replacement", 1, 42)
        );
    }

    #[test]
    fn rejects_non_directories_relative_roots_and_network_kinds() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file"), b"fixture").unwrap();
        for path in [
            Path::new("."),
            Path::new("/"),
            &temp.path().join("file"),
            &temp.path().join("missing"),
        ] {
            assert!(inspect(path).is_err());
        }
        for kind in [0x6969, 0x517b, 0xff534d42, 0xfe534d42] {
            assert!(linux_network(kind));
        }
        assert!(!linux_network(0xef53));
        #[cfg(windows)]
        for path in [
            r"\\server\share\folder",
            r"\\?\UNC\server\share\folder",
            r"\\.\C:\folder",
            r"C:\",
        ] {
            assert_eq!(
                inspect(Path::new(path)).unwrap_err().to_string(),
                "PROJECT_ROOT_UNSUPPORTED"
            );
        }
    }

    #[test]
    fn equivalent_paths_and_links_share_identity_but_replacement_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ProjectTea");
        std::fs::create_dir_all(root.join("child")).unwrap();
        let first = inspect(&root).unwrap();
        assert_eq!(inspect(&root.join("child/..")).unwrap(), first);
        assert_eq!(inspect(&root.join("")).unwrap(), first);
        assert!(is_within(&inspect(&root.join("child")).unwrap(), &root).unwrap());
        assert!(!is_within(&inspect(temp.path()).unwrap(), &root).unwrap());
        let case = temp.path().join("projecttea");
        let insensitive = case.exists();
        if insensitive {
            assert_eq!(inspect(&case).unwrap().identity, first.identity);
        }
        println!("M1_PROJECT_ROOT_IDENTITY_OK caseInsensitive={insensitive}");
        let link = temp.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, &link).unwrap();
        #[cfg(windows)]
        assert!(
            std::process::Command::new("cmd")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(&link)
                .arg(&root)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(inspect(&link).unwrap(), first);
        // Keep the old directory alive so inode/FileId reuse cannot make this test ambiguous.
        std::fs::rename(&root, temp.path().join("old")).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert_ne!(inspect(&root).unwrap().identity, first.identity);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_never_lossily_approved() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert_eq!(
            inspect(&path).unwrap_err().to_string(),
            "PROJECT_ROOT_NOT_UTF8"
        );
    }
    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_is_never_lossily_approved() {
        use std::os::windows::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_wide(&[67, 58, 92, 0xd800]));
        assert_eq!(
            inspect(&path).unwrap_err().to_string(),
            "PROJECT_ROOT_NOT_UTF8"
        );
    }
}
