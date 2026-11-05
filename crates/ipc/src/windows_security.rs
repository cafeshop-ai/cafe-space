//! The small Win32 boundary for private directories, files and named pipes.
#![allow(unsafe_code)]

use std::{
    ffi::c_void,
    fs::File,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        self, ACCESS_ALLOWED_ACE,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            ConvertStringSidToSidW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetTokenInformation,
        OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_ALWAYS, OPEN_EXISTING,
        READ_CONTROL,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct LocalMemory(*mut c_void);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns memory returned by Win32 LocalAlloc-based APIs.
        unsafe {
            LocalFree(self.0);
        }
    }
}

pub(crate) fn pipe_server_pid(handle: &impl AsRawHandle) -> io::Result<u32> {
    let mut pid = 0;
    // SAFETY: the borrowed pipe handle remains valid and pid points to writable storage.
    if unsafe {
        windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
            handle.as_raw_handle(),
            &mut pid,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

pub(crate) fn replace_durable(source: &Path, target: &Path) -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::ERROR_SHARING_VIOLATION,
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
    };
    let source = wide(source.as_os_str())?;
    let target = wide(target.as_os_str())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        // SAFETY: both paths are live NUL-terminated UTF-16 buffers; flags request durable replacement.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_SHARING_VIOLATION as i32)
            || std::time::Instant::now() >= deadline
        {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wide(value: &std::ffi::OsStr) -> io::Result<Vec<u16>> {
    let mut text: Vec<u16> = value.encode_wide().collect();
    if text.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in Windows path",
        ));
    }
    text.push(0);
    Ok(text)
}

pub(crate) fn current_sid() -> io::Result<String> {
    let mut token = ptr::null_mut();
    // SAFETY: the pseudo-process handle is valid; token points to writable handle storage.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned an owned non-null handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut length = 0;
    // SAFETY: zero-length query obtains the necessary buffer size without dereferencing null.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            ptr::null_mut(),
            0,
            &mut length,
        );
    }
    if length < size_of::<TOKEN_USER>() as u32 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    // SAFETY: the word-aligned buffer has at least length bytes; the owned token is live.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call initialized TOKEN_USER and its SID inside the live buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = ptr::null_mut();
    // SAFETY: the SID is valid and text points to writable output-pointer storage.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _text = LocalMemory(text.cast());
    let mut length = 0;
    // SAFETY: ConvertSidToStringSidW returned a NUL-terminated UTF-16 allocation.
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    // SAFETY: length was bounded by the allocation's NUL terminator above.
    String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid user SID"))
}

pub(crate) struct OwnerSecurity(LocalMemory);
impl OwnerSecurity {
    pub(crate) fn new(inherit: bool) -> io::Result<Self> {
        let sid = current_sid()?;
        let flags = if inherit { "OICI" } else { "" };
        let text = wide(std::ffi::OsStr::new(&format!(
            "O:{sid}D:P(A;{flags};FA;;;{sid})"
        )))?;
        let mut descriptor = ptr::null_mut();
        // SAFETY: text is NUL-terminated UTF-16, and descriptor is valid output storage.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(LocalMemory(descriptor)))
    }

    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: 0,
        }
    }
}

pub(crate) fn directory(path: &Path) -> io::Result<File> {
    let path = wide(path.as_os_str())?;
    let security = OwnerSecurity::new(true)?;
    // SAFETY: path and security attributes are valid for the duration of the synchronous call.
    if unsafe { CreateDirectoryW(path.as_ptr(), &security.attributes()) } == 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(e);
        }
    }
    open(&path, true, false, &security)
}

pub(crate) fn existing_directory(path: &Path) -> io::Result<File> {
    open(
        &wide(path.as_os_str())?,
        true,
        false,
        &OwnerSecurity::new(true)?,
    )
}

pub(crate) fn file(path: &Path) -> io::Result<File> {
    open(
        &wide(path.as_os_str())?,
        false,
        true,
        &OwnerSecurity::new(false)?,
    )
}

pub(crate) fn existing_file(path: &Path) -> io::Result<File> {
    open(
        &wide(path.as_os_str())?,
        false,
        false,
        &OwnerSecurity::new(false)?,
    )
}

fn open(
    path: &[u16],
    directory: bool,
    writable: bool,
    security: &OwnerSecurity,
) -> io::Result<File> {
    // SAFETY: path and attributes outlive the call. OPEN_REPARSE_POINT avoids following the final link.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_CONTROL | GENERIC_READ | if writable { GENERIC_WRITE } else { 0 },
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &security.attributes(),
            if writable { OPEN_ALWAYS } else { OPEN_EXISTING },
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned an owned handle; File closes it on every return path.
    let file = unsafe { File::from_raw_handle(handle) };
    use std::os::windows::fs::MetadataExt;
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid private path type",
        ));
    }
    if !directory {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the file handle is live and info is writable output storage.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.nNumberOfLinks != 1 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private files cannot have hard links",
            ));
        }
    }
    check_owner_only(&file)?;
    Ok(file)
}

pub(crate) fn check_owner_only(file: &impl AsRawHandle) -> io::Result<()> {
    let mut owner = ptr::null_mut();
    let mut acl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    // SAFETY: the handle is live and every requested output is backed by writable storage.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let _descriptor = LocalMemory(descriptor);
    let sid_text = wide(std::ffi::OsStr::new(&current_sid()?))?;
    let mut sid = ptr::null_mut();
    // SAFETY: sid_text is NUL-terminated; sid is output-pointer storage.
    if unsafe { ConvertStringSidToSidW(sid_text.as_ptr(), &mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _sid = LocalMemory(sid);
    // SAFETY: GetSecurityInfo returned owner/ACL pointers inside the live descriptor; null is checked first.
    let valid = unsafe {
        if owner.is_null() || acl.is_null() || EqualSid(owner, sid) == 0 || (*acl).AceCount != 1 {
            false
        } else {
            let mut ace = ptr::null_mut();
            if GetAce(acl, 0, &mut ace) == 0 || ace.is_null() {
                false
            } else {
                let header = &*ace.cast::<Security::ACE_HEADER>();
                if header.AceType != 0
                    || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
                {
                    false
                } else {
                    let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    allowed.Header.AceFlags & Security::INHERIT_ONLY_ACE as u8 == 0
                        && allowed.Mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS
                        && EqualSid(ptr::addr_of!(allowed.SidStart).cast_mut().cast(), sid) != 0
                }
            }
        }
    };
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "state path must have an owner-only DACL",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_an_existing_public_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("public");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            directory(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn refuses_hard_links_and_junctions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");
        let _guard = directory(&path).unwrap();
        let original = path.join("original");
        let _file = file(&original).unwrap();
        std::fs::hard_link(&original, path.join("hardlink")).unwrap();
        assert!(file(&original).is_err());
        let junction = temp.path().join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success(), "could not create test junction");
        assert!(directory(&junction).is_err());
        std::fs::remove_dir(junction).unwrap();
    }

    #[test]
    fn live_private_directory_cannot_be_renamed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");
        let guard = directory(&path).unwrap();
        assert!(std::fs::rename(&path, temp.path().join("moved")).is_err());
        drop(guard);
        std::fs::rename(&path, temp.path().join("moved")).unwrap();
    }
}
