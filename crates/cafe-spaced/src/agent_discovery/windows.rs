//! Read the current registry PATH without executing a shell or changing the process environment.
#![allow(unsafe_code)]
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};
use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
        RegGetValueW,
    },
};

fn read_path(key: HKEY, subkey: &str) -> Option<OsString> {
    let subkey: Vec<_> = subkey.encode_utf16().chain([0]).collect();
    let name: Vec<_> = "Path".encode_utf16().chain([0]).collect();
    // Fixed allocation bounds both registry expansion and reads, including concurrent PATH edits.
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: predefined borrowed key, terminated strings, and a live byte-sized output buffer.
    let result = unsafe {
        RegGetValueW(
            key,
            subkey.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result != ERROR_SUCCESS
        || bytes < 2
        || bytes as usize > buffer.len() * 2
        || !bytes.is_multiple_of(2)
    {
        return None;
    }
    let value = &buffer[..bytes as usize / 2];
    if value.last() != Some(&0) || value[..value.len() - 1].contains(&0) {
        return None;
    }
    Some(OsString::from_wide(&value[..value.len() - 1]))
}

pub(super) fn registry_paths() -> Vec<PathBuf> {
    [
        (
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        ),
        (HKEY_CURRENT_USER, "Environment"),
    ]
    .into_iter()
    .filter_map(|(key, subkey)| read_path(key, subkey))
    .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
    .filter(|path| super::is_local_path(path))
    .take(256)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lookup_is_read_only_and_keeps_only_local_absolute_directories() {
        let before = std::env::var_os("PATH");
        let paths = registry_paths();
        assert!(paths.len() <= 256);
        assert!(paths.iter().all(|path| super::super::is_local_path(path)));
        assert_eq!(std::env::var_os("PATH"), before);
    }
}
