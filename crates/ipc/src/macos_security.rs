//! Darwin ACLs can grant access that is not reflected in POSIX mode bits.
#![allow(unsafe_code)]

use std::{ffi::c_void, fs::File, io, os::fd::AsRawFd, ptr};

// Darwin Libc include/sys/acl.h and posix1e/acl_entry.c (not Linux libacl semantics).
unsafe extern "C" {
    fn acl_get_fd(fd: i32) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, entry_id: i32, entry: *mut *mut c_void) -> i32;
    fn acl_free(acl: *mut c_void) -> i32;
}

pub(crate) fn require_no_acl(file: &File) -> io::Result<()> {
    // SAFETY: file owns a live descriptor; acl_get_fd allocates an independent ACL copy.
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        // A live descriptor with no FILESEC_ACL property reports ENOENT on Darwin.
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(())
        } else {
            Err(error)
        };
    }
    let mut entry = ptr::null_mut();
    // SAFETY: acl is valid and entry is output-pointer storage. Darwin ACL_FIRST_ENTRY = 0.
    let status = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = io::Error::last_os_error();
    // SAFETY: this function owns the ACL and no pointers into it are used after freeing.
    unsafe {
        acl_free(acl);
    }
    match status {
        -1 if error.raw_os_error() == Some(libc::EINVAL) => Ok(()), // Darwin's empty ACL.
        -1 => Err(error),
        _ => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private state paths must not have extended ACLs",
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_extended_acl_even_with_mode_0700() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private");
        let guard = crate::private::directory(&path).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read,search"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(crate::private::directory(&path).is_err());
        drop(guard);
    }
}
