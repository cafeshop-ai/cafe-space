//! Values are private, including proxy credentials and CA paths. Never format this environment.
use super::error;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
};

const INHERITED: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SystemRoot",
    "COMSPEC",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "SystemDrive",
    "windir",
    "PATHEXT",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramData",
    "CommonProgramFiles",
    "USERNAME",
    "USERDOMAIN",
    "HOMEDRIVE",
    "HOMEPATH",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "SHELL",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "all_proxy",
];

const OWNER_ONLY: &[&str] = &["NODE_EXTRA_CA_CERTS", "SSL_CERT_FILE", "SSH_AUTH_SOCK"];

/// Explicit overrides come only from local-owner settings, never a client command or agent output.
/// Proxy values remain private; custom CAs and SSH signing require explicit owner overrides.
pub struct Environment(Vec<(OsString, OsString)>);
impl Environment {
    pub fn inherited() -> io::Result<Self> {
        Self::from_lookup(|name| std::env::var_os(name), std::iter::empty())
    }
    pub fn for_owner(
        overrides: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        Self::from_lookup(|name| std::env::var_os(name), overrides)
    }
    fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<OsString>,
        overrides: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        let mut values = BTreeMap::new();
        for name in INHERITED {
            if let Some(value) = lookup(name) {
                values.insert(key(OsStr::new(name)), value);
            }
        }
        for (index, (name, value)) in overrides.into_iter().enumerate() {
            let name = key(&name);
            if index >= INHERITED.len() + OWNER_ONLY.len()
                || !INHERITED
                    .iter()
                    .chain(OWNER_ONLY)
                    .any(|allowed| key(OsStr::new(allowed)) == name)
            {
                return Err(error("PROCESS_ENVIRONMENT_UNSUPPORTED"));
            }
            values.insert(name, value);
        }
        if values.iter().any(|(name, value)| {
            name.as_encoded_bytes().contains(&0) || value.as_encoded_bytes().contains(&0)
        }) || values
            .iter()
            .map(|(name, value)| name.len() + value.len() + 2)
            .sum::<usize>()
            > 65536
        {
            return Err(error("PROCESS_ENVIRONMENT_INVALID"));
        }
        Ok(Self(values.into_iter().collect()))
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
    /// Bounded file discovery only; never execute a tool or search cwd/PATHEXT from the caller.
    pub(crate) fn tool_found(&self, tool: &str) -> bool {
        if !["git", "node", "npm", "pwsh"].contains(&tool) || self.require_path().is_err() {
            return false;
        }
        let Some((_, value)) = self
            .0
            .iter()
            .find(|(name, _)| name == &key(OsStr::new("PATH")))
        else {
            return false;
        };
        let directories: Vec<_> = std::env::split_paths(value).take(257).collect();
        if directories.len() > 256 {
            return false;
        }
        directories.iter().any(|directory| {
            #[cfg(windows)]
            if directory.to_string_lossy().starts_with(r"\") {
                return false;
            }
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
    fn inherited_environment_preserves_proxies_but_excludes_ca_signing_and_tokens() {
        let environment =
            Environment::from_lookup(|_| Some("synthetic-private".into()), std::iter::empty())
                .unwrap();
        for name in ["PATH", "PATHEXT", "SystemDrive", "XDG_CONFIG_HOME", "SHELL"] {
            assert!(environment.0.iter().any(
                |(key_, value)| key_ == &key(OsStr::new(name)) && value == "synthetic-private"
            ));
        }
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/process-ownership/contract.json"
        ))
        .unwrap();
        for name in contract["environment"]["inheritedProxyNames"]
            .as_array()
            .unwrap()
        {
            assert!(environment.0.contains(&(
                key(OsStr::new(name.as_str().unwrap())),
                "synthetic-private".into(),
            )));
        }
        for name in [
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "SSH_AUTH_SOCK",
            "API_KEY",
            "OPENAI_API_KEY",
            "CAFE_SPACE_TOKEN",
            "PI_COLLAB_HOST_TOKEN",
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
        ] {
            assert!(
                !environment
                    .0
                    .iter()
                    .any(|(key_, _)| key_ == &key(OsStr::new(name)))
            );
        }
    }
    #[test]
    fn proxy_overrides_replace_defaults_without_manufacturing_missing_values() {
        let lookup = |name: &str| match name {
            "HTTPS_PROXY" | "https_proxy" => Some("http://inherited.invalid:3128".into()),
            "NO_PROXY" | "no_proxy" => Some("localhost,.internal".into()),
            _ => None,
        };
        let inherited = Environment::from_lookup(lookup, std::iter::empty()).unwrap();
        let overridden = Environment::from_lookup(
            lookup,
            [("HTTPS_PROXY".into(), "http://override.invalid:8080".into())],
        )
        .unwrap();
        assert!(overridden.0.contains(&(
            key(OsStr::new("HTTPS_PROXY")),
            "http://override.invalid:8080".into(),
        )));
        assert!(
            overridden
                .0
                .contains(&(key(OsStr::new("NO_PROXY")), "localhost,.internal".into(),))
        );
        assert!(inherited.0.contains(&(
            key(OsStr::new("HTTPS_PROXY")),
            "http://inherited.invalid:3128".into(),
        )));
        assert!(
            !overridden
                .0
                .iter()
                .any(|(name, _)| name == "HTTP_PROXY" || name == "http_proxy")
        );
        #[cfg(unix)]
        assert!(
            overridden
                .0
                .contains(&("https_proxy".into(), "http://inherited.invalid:3128".into(),))
        );
        assert!(
            Environment::from_lookup(|_| None, std::iter::empty())
                .unwrap()
                .0
                .is_empty()
        );
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
            let before =
                Environment::from_lookup(|_| None, [("PATH".into(), path.clone())]).unwrap();
            assert!(!before.tool_found("git"));
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let environment =
            Environment::from_lookup(|_| None, [("PATH".into(), path.clone())]).unwrap();
        assert!(environment.tool_found("git"));
        assert!(!environment.tool_found("node"));
        assert!(!environment.tool_found("../git"));
        let too_many = std::env::join_paths(std::iter::repeat_n(path, 257)).unwrap();
        let environment = Environment::from_lookup(|_| None, [("PATH".into(), too_many)]).unwrap();
        assert!(!environment.tool_found("git"));
        let cwd = Environment::from_lookup(|_| None, [("PATH".into(), ".".into())]).unwrap();
        assert!(!cwd.tool_found("git"));
    }

    #[test]
    fn missing_empty_relative_and_cwd_search_path_entries_are_not_ready() {
        for value in [None, Some(""), Some("."), Some("relative/bin")] {
            let environment = Environment::from_lookup(
                |name| {
                    if name == "PATH" {
                        value.map(OsString::from)
                    } else {
                        None
                    }
                },
                std::iter::empty(),
            )
            .unwrap();
            assert!(environment.require_path().is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let absolute = temp.path().as_os_str().to_owned();
        let environment =
            Environment::from_lookup(|_| None, [("PATH".into(), absolute.clone())]).unwrap();
        assert!(environment.require_path().is_ok());
        let unsafe_search = std::env::join_paths([absolute.clone(), OsString::new()]).unwrap();
        let environment =
            Environment::from_lookup(|_| None, [("PATH".into(), unsafe_search)]).unwrap();
        assert!(environment.require_path().is_err());
        let environment =
            Environment::from_lookup(|_| Some(".".into()), [("PATH".into(), absolute)]).unwrap();
        assert!(
            environment.require_path().is_ok(),
            "explicit owner PATH replaces, not merges, the unsafe inherited PATH"
        );
    }

    #[test]
    fn only_owner_overrides_enable_signing_and_neither_overrides_nor_values_bypass_bounds() {
        let source = [
            ("SSH_AUTH_SOCK".into(), "synthetic-signing-socket".into()),
            ("PATH".into(), "synthetic-owner-path".into()),
            ("HTTPS_PROXY".into(), "synthetic-proxy".into()),
            ("SSL_CERT_FILE".into(), "synthetic-ca-file".into()),
        ];
        let environment = Environment::from_lookup(|_| None, source).unwrap();
        assert!(
            environment
                .0
                .contains(&("SSH_AUTH_SOCK".into(), "synthetic-signing-socket".into()))
        );
        assert!(
            environment
                .0
                .contains(&("HTTPS_PROXY".into(), "synthetic-proxy".into()))
        );
        assert!(
            environment
                .0
                .contains(&("SSL_CERT_FILE".into(), "synthetic-ca-file".into()))
        );
        for (name, value, code) in [
            (
                "NODE_OPTIONS",
                "--require /secret/path",
                "PROCESS_ENVIRONMENT_UNSUPPORTED",
            ),
            (
                "CAFE_SPACE_TOKEN",
                "synthetic-secret",
                "PROCESS_ENVIRONMENT_UNSUPPORTED",
            ),
            ("PATH", "has\0nul", "PROCESS_ENVIRONMENT_INVALID"),
        ] {
            assert_eq!(
                Environment::from_lookup(|_| None, [(name.into(), value.into())])
                    .err()
                    .unwrap()
                    .to_string(),
                code
            );
        }
        assert!(
            Environment::from_lookup(|_| None, [("PATH".into(), "x".repeat(65537).into())])
                .is_err()
        );
        #[cfg(windows)]
        {
            let environment = Environment::from_lookup(
                |_| None,
                [
                    ("Path".into(), "first".into()),
                    ("PATH".into(), "last".into()),
                ],
            )
            .unwrap();
            assert_eq!(environment.0, vec![("PATH".into(), "last".into())]);
        }
    }
}
