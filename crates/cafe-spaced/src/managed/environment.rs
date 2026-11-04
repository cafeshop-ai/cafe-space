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
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "all_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
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
];

/// Explicit overrides come only from local-owner settings, never a client command or agent output.
/// SSH_AUTH_SOCK is opt-in: it delegates ssh-agent signing authority, not just a path.
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
            if index > INHERITED.len()
                || !(name == "SSH_AUTH_SOCK"
                    || INHERITED
                        .iter()
                        .any(|allowed| key(OsStr::new(allowed)) == name))
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
    fn reviewed_allowlist_preserves_platform_proxy_and_ca_values_but_never_ambient_signing_or_tokens()
     {
        let environment =
            Environment::from_lookup(|_| Some("synthetic-private".into()), std::iter::empty())
                .unwrap();
        for name in [
            "HTTPS_PROXY",
            "http_proxy",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "PATH",
            "PATHEXT",
            "SystemDrive",
            "XDG_CONFIG_HOME",
            "SHELL",
        ] {
            assert!(environment.0.iter().any(
                |(key_, value)| key_ == &key(OsStr::new(name)) && value == "synthetic-private"
            ));
        }
        for name in [
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
    fn only_owner_overrides_enable_signing_and_neither_overrides_nor_values_bypass_bounds() {
        let source = [
            ("SSH_AUTH_SOCK".into(), "synthetic-signing-socket".into()),
            ("PATH".into(), "synthetic-owner-path".into()),
        ];
        let environment = Environment::from_lookup(|_| None, source).unwrap();
        assert!(
            environment
                .0
                .contains(&("SSH_AUTH_SOCK".into(), "synthetic-signing-socket".into()))
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
