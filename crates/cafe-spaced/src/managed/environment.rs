//! Values are private, including proxy credentials and CA paths. Never format this environment.
use super::error;
use std::{
    cmp::Ordering,
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
};

/// D20: inherit the daemon environment, except names reserved for the current Café session.
/// Values never implement Debug or leave the private child-launch path.
pub struct Environment(Vec<(OsString, OsString)>);
impl Environment {
    pub fn inherited() -> io::Result<Self> {
        Self::from_values(std::env::vars_os(), std::iter::empty())
    }
    pub fn for_owner(
        overrides: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        Self::from_values(std::env::vars_os(), overrides)
    }
    fn from_values(
        inherited: impl IntoIterator<Item = (OsString, OsString)>,
        overrides: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        let mut values: Vec<_> = inherited
            .into_iter()
            .filter(|(name, _)| {
                !name
                    .as_encoded_bytes()
                    .get(..11)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"CAFE_SPACE_"))
            })
            .map(|(name, value)| (key(&name), value))
            .collect();
        let mut override_bytes = 0;
        for (index, (name, value)) in overrides.into_iter().enumerate() {
            if index >= cafe_space_protocol::MAX_OWNER_ENVIRONMENT_ENTRIES
                || !name
                    .to_str()
                    .is_some_and(cafe_space_protocol::valid_owner_environment_name)
            {
                return Err(error("PROCESS_ENVIRONMENT_UNSUPPORTED"));
            }
            override_bytes += name.len() + value.len() + 2;
            if override_bytes > 65536 || value.as_encoded_bytes().contains(&0) {
                return Err(error("PROCESS_ENVIRONMENT_INVALID"));
            }
            #[cfg(unix)]
            if cafe_space_protocol::owner_proxy_name(name.to_str().unwrap()).is_some() {
                values.push((name.to_ascii_uppercase(), value.clone()));
                values.push((name.to_ascii_lowercase(), value.clone()));
            }
            values.push((key(&name), value));
        }
        // Stable sorting keeps the last write first among equivalent names.
        values.reverse();
        values.sort_by(|(left, _), (right, _)| compare_names(left, right));
        values.dedup_by(|(left, _), (right, _)| compare_names(left, right).is_eq());
        Ok(Self(values))
    }
    /// Readiness is structural only: never guess a PATH, search the cwd or import a shell.
    pub fn require_path(&self) -> io::Result<()> {
        let path = self
            .0
            .iter()
            .find(|(name, _)| name == &key(OsStr::new("PATH")))
            .map(|(_, value)| value)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| error("OWNER_PATH_REQUIRED"))?;
        if std::env::split_paths(path).any(|entry| !entry.is_absolute()) {
            return Err(error("OWNER_PATH_INVALID"));
        }
        Ok(())
    }
    pub(crate) fn path_entries(&self) -> Vec<PathBuf> {
        if self.require_path().is_err() {
            return vec![];
        }
        let Some((_, value)) = self
            .0
            .iter()
            .find(|(name, _)| name == &key(OsStr::new("PATH")))
        else {
            return vec![];
        };
        let directories: Vec<_> = std::env::split_paths(value).take(257).collect();
        if directories.len() > 256 {
            return vec![];
        }
        directories
            .into_iter()
            .filter(|directory| {
                #[cfg(windows)]
                return !directory.to_string_lossy().starts_with(r"\");
                #[cfg(not(windows))]
                directory.is_absolute()
            })
            .collect()
    }

    /// Bounded file discovery only; never execute a tool or search cwd/PATHEXT from the caller.
    pub(crate) fn tool_found(&self, tool: &str) -> bool {
        if !["git", "node", "npm", "pwsh"].contains(&tool) {
            return false;
        }
        self.path_entries().iter().any(|directory| {
            let extensions: &[&str] = if cfg!(windows) {
                &[".exe", ".cmd", ".bat", ".com"]
            } else {
                &[""]
            };
            extensions.iter().any(|extension| {
                let Ok(metadata) = std::fs::metadata(directory.join(format!("{tool}{extension}")))
                else {
                    return false;
                };
                if !metadata.is_file() {
                    return false;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(windows)]
                {
                    true
                }
            })
        })
    }

    pub(super) fn into_values(self) -> Vec<(OsString, OsString)> {
        self.0
    }
}
pub(crate) fn compare_names(left: &OsStr, right: &OsStr) -> Ordering {
    #[cfg(windows)]
    {
        super::windows::compare_environment_names(left, right)
    }
    #[cfg(not(windows))]
    {
        left.cmp(right)
    }
}

fn key(name: &OsStr) -> OsString {
    #[cfg(windows)]
    {
        name.to_ascii_uppercase()
    }
    #[cfg(not(windows))]
    {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_daemon_environment_is_preserved_except_reserved_session_names() {
        let names = [
            "PATH",
            "HOME",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "SSH_AUTH_SOCK",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "PI_COLLAB_HOST_TOKEN",
            "CUSTOM_TOKEN",
        ];
        let source = names
            .into_iter()
            .chain(["CAFE_SPACE_TOKEN", "cafe_space_old"])
            .map(|name| (name.into(), "synthetic-private".into()));
        let environment = Environment::from_values(source, []).unwrap();
        for name in names {
            assert!(
                environment
                    .0
                    .contains(&(key(OsStr::new(name)), "synthetic-private".into()))
            );
        }
        assert_eq!(environment.0.len(), names.len());
        // The former override budget must not truncate a valid inherited environment.
        assert!(Environment::from_values([("LARGE".into(), "x".repeat(65537).into())], []).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_daemon_values_are_inherited_losslessly() {
        use std::os::unix::ffi::OsStringExt;
        let pair = (
            OsString::from_vec(vec![b'X', 0xff]),
            OsString::from_vec(vec![0xfe]),
        );
        let environment = Environment::from_values([pair.clone()], []).unwrap();
        assert_eq!(environment.0, vec![pair]);
    }

    #[test]
    fn proxy_overrides_replace_both_spellings_without_manufacturing_unrelated_proxies() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/process-ownership/contract.json"
        ))
        .unwrap();
        for proxy in contract["environment"]["inheritedProxyNames"]
            .as_array()
            .unwrap()
        {
            let proxy = proxy.as_str().unwrap();
            let source = [
                (proxy.to_uppercase().into(), "upper-default".into()),
                (proxy.to_lowercase().into(), "lower-default".into()),
            ];
            let inherited = Environment::from_values(source.clone(), []).unwrap();
            let overridden =
                Environment::from_values(source, [(proxy.into(), "override".into())]).unwrap();
            for alias in [proxy.to_uppercase(), proxy.to_lowercase()] {
                assert!(
                    overridden
                        .0
                        .contains(&(key(OsStr::new(&alias)), "override".into()))
                );
            }
            #[cfg(unix)]
            {
                assert!(
                    inherited
                        .0
                        .contains(&(proxy.to_uppercase().into(), "upper-default".into()))
                );
                assert!(
                    inherited
                        .0
                        .contains(&(proxy.to_lowercase().into(), "lower-default".into()))
                );
            }
            assert_eq!(overridden.0.len(), inherited.0.len());
        }
        assert!(Environment::from_values([], []).unwrap().0.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn unicode_names_merge_last_write_wins_and_sort_in_windows_ordinal_order() {
        for (inherited, overridden) in [("äKEY", "ÄKEY"), ("ÄKEY", "äKEY")] {
            let source = [
                (inherited.into(), "inherited".into()),
                ("ÖKEY".into(), "other".into()),
            ];
            let environment =
                Environment::from_values(source.clone(), [(overridden.into(), "override".into())])
                    .unwrap();
            assert_eq!(
                environment.0,
                vec![
                    (overridden.into(), "override".into()),
                    ("ÖKEY".into(), "other".into())
                ]
            );
            let restored = Environment::from_values(source, []).unwrap();
            assert_eq!(
                restored.0,
                vec![
                    (inherited.into(), "inherited".into()),
                    ("ÖKEY".into(), "other".into())
                ]
            );
            let repeated = Environment::from_values(
                [],
                [
                    (inherited.into(), "first".into()),
                    (overridden.into(), "last".into()),
                ],
            )
            .unwrap();
            assert_eq!(repeated.0, vec![(overridden.into(), "last".into())]);
        }
    }

    #[test]
    fn tool_discovery_is_bounded_file_only_and_never_searches_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().as_os_str().to_owned();
        let tool = temp
            .path()
            .join(if cfg!(windows) { "git.exe" } else { "git" });
        std::fs::write(&tool, "synthetic fixture, never execute").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let before = Environment::from_values([], [("PATH".into(), path.clone())]).unwrap();
            assert!(!before.tool_found("git"));
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let environment = Environment::from_values([], [("PATH".into(), path.clone())]).unwrap();
        assert!(environment.tool_found("git"));
        assert!(!environment.tool_found("node"));
        assert!(!environment.tool_found("../git"));
        let too_many = std::env::join_paths(std::iter::repeat_n(path, 257)).unwrap();
        assert!(
            !Environment::from_values([], [("PATH".into(), too_many)])
                .unwrap()
                .tool_found("git")
        );
        assert!(
            !Environment::from_values([], [("PATH".into(), ".".into())])
                .unwrap()
                .tool_found("git")
        );
    }

    #[test]
    fn missing_empty_relative_and_cwd_search_path_entries_are_not_ready() {
        assert!(
            Environment::from_values([], [])
                .unwrap()
                .require_path()
                .is_err()
        );
        for value in ["", ".", "relative/bin"] {
            let environment =
                Environment::from_values([("PATH".into(), value.into())], []).unwrap();
            assert!(environment.require_path().is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let absolute = temp.path().as_os_str().to_owned();
        let environment =
            Environment::from_values([], [("PATH".into(), absolute.clone())]).unwrap();
        assert!(environment.require_path().is_ok());
        let unsafe_search = std::env::join_paths([absolute.clone(), OsString::new()]).unwrap();
        assert!(
            Environment::from_values([], [("PATH".into(), unsafe_search)])
                .unwrap()
                .require_path()
                .is_err()
        );
        assert!(
            Environment::from_values([("PATH".into(), ".".into())], [("PATH".into(), absolute)])
                .unwrap()
                .require_path()
                .is_ok()
        );
    }

    #[test]
    fn owner_overrides_win_but_reserved_names_and_invalid_values_are_rejected() {
        for name in ["OPENAI_API_KEY", "SSH_AUTH_SOCK", "NODE_OPTIONS", "CUSTOM"] {
            let environment = Environment::from_values(
                [(name.into(), "inherited".into())],
                [(name.into(), "override".into())],
            )
            .unwrap();
            assert_eq!(environment.0, vec![(name.into(), "override".into())]);
        }
        for name in [
            "CAFE_SPACE_TOKEN",
            "cafe_space_token",
            "",
            "A=B",
            "bad\nname",
        ] {
            assert_eq!(
                Environment::from_values([], [(name.into(), "private".into())])
                    .err()
                    .unwrap()
                    .to_string(),
                "PROCESS_ENVIRONMENT_UNSUPPORTED"
            );
        }
        for value in ["has\0nul".to_owned(), "x".repeat(65537)] {
            assert!(Environment::from_values([], [("PATH".into(), value.into())]).is_err());
        }
        assert!(
            Environment::from_values(
                [],
                (0..257).map(|i| (format!("KEY_{i}").into(), "v".into()))
            )
            .is_err()
        );
        #[cfg(windows)]
        {
            let inherited = [("Path".into(), "original".into())];
            let environment = Environment::from_values(
                inherited.clone(),
                [
                    ("Path".into(), "first".into()),
                    ("PATH".into(), "last".into()),
                ],
            )
            .unwrap();
            assert_eq!(environment.0, vec![("PATH".into(), "last".into())]);
            let restored = Environment::from_values(inherited, []).unwrap();
            assert_eq!(restored.0, vec![("PATH".into(), "original".into())]);
        }
    }
}
