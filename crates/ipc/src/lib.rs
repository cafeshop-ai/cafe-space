//! Shared local IPC framing, process flavors, and platform-specific data-directory paths.

pub mod framing;

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildFlavor {
    Release,
    Dev,
}

impl BuildFlavor {
    pub const fn directory_name(self) -> &'static str {
        match self {
            Self::Release => "cafe-space",
            Self::Dev => "cafe-space-dev",
        }
    }

    pub const fn daemon_name(self) -> &'static str {
        match self {
            Self::Release => "cafe-spaced",
            Self::Dev => "cafe-spaced-dev",
        }
    }

    pub const fn cli_name(self) -> &'static str {
        self.directory_name()
    }

    pub fn data_dir(self) -> Option<PathBuf> {
        platform_data_dir(self.directory_name())
    }
}

fn platform_data_dir(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(home_dir()?.join("Library/Application Support").join(name))
    }

    #[cfg(target_os = "windows")]
    {
        Some(absolute_path(std::env::var_os("LOCALAPPDATA"))?.join(name))
    }

    #[cfg(target_os = "linux")]
    {
        let data_home = absolute_path(std::env::var_os("XDG_DATA_HOME"))
            .or_else(|| home_dir().map(|home| home.join(".local/share")))?;
        Some(data_home.join(name))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = name;
        None
    }
}

fn absolute_path(path: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(path?);
    path.is_absolute().then_some(path)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn home_dir() -> Option<PathBuf> {
    absolute_path(std::env::var_os("HOME"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavors_have_unique_names_and_absolute_roots_are_required() {
        assert_eq!(BuildFlavor::Release.directory_name(), "cafe-space");
        assert_eq!(BuildFlavor::Release.daemon_name(), "cafe-spaced");
        assert_eq!(BuildFlavor::Release.cli_name(), "cafe-space");
        assert_eq!(BuildFlavor::Dev.directory_name(), "cafe-space-dev");
        assert_eq!(BuildFlavor::Dev.daemon_name(), "cafe-spaced-dev");
        assert_eq!(BuildFlavor::Dev.cli_name(), "cafe-space-dev");
        assert_ne!(
            BuildFlavor::Release.directory_name(),
            BuildFlavor::Dev.directory_name()
        );
        assert_eq!(
            absolute_path(Some(OsString::from("."))),
            None,
            "relative data roots must not create state in the working directory"
        );
        let absolute = std::env::current_dir().unwrap();
        assert_eq!(
            absolute_path(Some(absolute.clone().into_os_string())),
            Some(absolute)
        );
        if let (Some(release), Some(dev)) =
            (BuildFlavor::Release.data_dir(), BuildFlavor::Dev.data_dir())
        {
            assert!(release.is_absolute() && dev.is_absolute());
            assert_eq!(release.parent(), dev.parent());
            assert_eq!(
                release.file_name().and_then(|name| name.to_str()),
                Some("cafe-space")
            );
            assert_eq!(
                dev.file_name().and_then(|name| name.to_str()),
                Some("cafe-space-dev")
            );
        }
    }
}
