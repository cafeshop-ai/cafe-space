//! Private local-owner environment. Values are deliberately never Debug or protocol data.
use crate::managed::Environment;
use cafe_space_ipc::private;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    path::Path,
};

const MAX_BYTES: usize = 65536;
const FILE: &str = "owner-environment.json";

#[derive(Debug, PartialEq, Eq)]
pub struct OwnerEnvironmentStatus {
    pub overridden_names: Vec<String>,
    pub path_ready: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    version: u32,
    overrides: BTreeMap<String, String>,
}
fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}
fn name(value: &str) -> String {
    #[cfg(windows)]
    {
        value.to_ascii_uppercase()
    }
    #[cfg(not(windows))]
    {
        value.into()
    }
}
impl Settings {
    fn environment(&self) -> io::Result<Environment> {
        Environment::for_owner(
            self.overrides
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        )
    }
    fn status(&self) -> io::Result<OwnerEnvironmentStatus> {
        Ok(OwnerEnvironmentStatus {
            overridden_names: self.overrides.keys().cloned().collect(),
            path_ready: self.environment()?.require_path().is_ok(),
        })
    }
}
fn load(root: &Path) -> io::Result<Settings> {
    let _guard =
        private::existing_directory(root).map_err(|_| error("OWNER_SETTINGS_UNTRUSTED"))?;
    let file = match private::existing_file(&root.join(FILE)) {
        Ok(file) => file,
        Err(value) if value.kind() == io::ErrorKind::NotFound => {
            return Ok(Settings {
                version: 1,
                overrides: BTreeMap::new(),
            });
        }
        Err(_) => return Err(error("OWNER_SETTINGS_UNTRUSTED")),
    };
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("OWNER_SETTINGS_READ_FAILED"))?;
    if bytes.len() > MAX_BYTES {
        return Err(error("OWNER_SETTINGS_INVALID"));
    }
    let settings: Settings =
        serde_json::from_slice(&bytes).map_err(|_| error("OWNER_SETTINGS_INVALID"))?;
    if settings.version != 1 {
        return Err(error("OWNER_SETTINGS_INVALID"));
    }
    let mut overrides = BTreeMap::new();
    for (key, value) in settings.overrides {
        if overrides.insert(name(&key), value).is_some() {
            return Err(error("OWNER_SETTINGS_INVALID"));
        }
    }
    let settings = Settings {
        version: 1,
        overrides,
    };
    settings.environment()?;
    Ok(settings)
}

pub(crate) fn status(root: &Path) -> io::Result<OwnerEnvironmentStatus> {
    load(root)?.status()
}

pub(crate) fn environment(root: &Path) -> io::Result<Environment> {
    let environment = load(root)?.environment()?;
    environment.require_path()?;
    Ok(environment)
}

/// Host owns the admission mutex and state directory lock. No write result is replayed.
pub(crate) fn patch(
    root: &Path,
    changes: BTreeMap<String, Option<String>>,
) -> io::Result<OwnerEnvironmentStatus> {
    // Removal names must pass the same allowlist as additions.
    Environment::for_owner(changes.iter().map(|(name, value)| {
        (
            OsString::from(name),
            OsString::from(value.as_deref().unwrap_or("")),
        )
    }))?;
    let mut settings = load(root)?;
    let mut normalized = BTreeMap::new();
    for (key, value) in changes {
        if normalized.insert(name(&key), value).is_some() {
            return Err(error("OWNER_SETTINGS_INVALID"));
        }
    }
    for (key, value) in normalized {
        match value {
            Some(value) => {
                settings.overrides.insert(key, value);
            }
            None => {
                settings.overrides.remove(&key);
            }
        }
    }
    let environment = settings.environment()?;
    environment.require_path()?;
    let bytes = serde_json::to_vec(&settings).map_err(|_| error("OWNER_SETTINGS_INVALID"))?;
    if bytes.len() > MAX_BYTES {
        return Err(error("OWNER_SETTINGS_INVALID"));
    }
    let status = settings.status()?;
    let _guard =
        private::existing_directory(root).map_err(|_| error("OWNER_SETTINGS_UNTRUSTED"))?;
    let temporary = root.join("owner-environment.next.json");
    let mut file = private::file(&temporary).map_err(|_| error("OWNER_SETTINGS_UNTRUSTED"))?;
    let written = file
        .set_len(0)
        .and_then(|()| file.write_all(&bytes))
        .and_then(|()| file.sync_all());
    drop(file);
    written
        .and_then(|()| private::replace_durable(&temporary, &root.join(FILE)))
        .map_err(|_| error("OWNER_SETTINGS_WRITE_UNKNOWN"))?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        drop(private::directory(&root).unwrap());
        (temp, root)
    }
    fn change(key: &str, value: Option<&str>) -> BTreeMap<String, Option<String>> {
        [(key.into(), value.map(String::from))].into()
    }
    #[test]
    fn private_settings_patch_survives_reopen_and_never_exports_values() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/process-ownership/owner-environment.json"
        ))
        .unwrap();
        assert_eq!(contract["maxEncodedBytes"], MAX_BYTES);
        assert_eq!(contract["file"], FILE);
        let (_temp, root) = fixture();
        assert!(status(&root).unwrap().overridden_names.is_empty());
        assert!(!root.join(FILE).exists());
        let proxy = "http://synthetic-user:synthetic-password@proxy.invalid:3128";
        let first = patch(&root, change("HTTPS_PROXY", Some(proxy))).unwrap();
        assert_eq!(first.overridden_names, ["HTTPS_PROXY"]);
        assert!(!format!("{first:?}").contains("synthetic-password"));
        assert_eq!(load(&root).unwrap().overrides["HTTPS_PROXY"], proxy);
        patch(
            &root,
            change("SSH_AUTH_SOCK", Some("synthetic-signing-socket")),
        )
        .unwrap();
        patch(&root, change("HTTPS_PROXY", None)).unwrap();
        let reloaded = load(&root).unwrap();
        assert!(!reloaded.overrides.contains_key("HTTPS_PROXY"));
        assert_eq!(
            reloaded.overrides["SSH_AUTH_SOCK"],
            "synthetic-signing-socket"
        );
        assert!(private::existing_file(&root.join(FILE)).is_ok());
    }
    #[test]
    fn rejected_settings_preserve_bytes_and_never_expose_bad_input() {
        let (_temp, root) = fixture();
        patch(&root, change("NO_PROXY", Some("synthetic.invalid"))).unwrap();
        let before = std::fs::read(root.join(FILE)).unwrap();
        for key in [
            "NODE_OPTIONS",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "OPENAI_API_KEY",
            "CAFE_SPACE_TOKEN",
        ] {
            for value in [Some("synthetic-secret"), None] {
                let failure = patch(&root, change(key, value)).unwrap_err();
                assert!(!failure.to_string().contains("synthetic-secret"));
                assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
            }
        }
        for value in [
            "has\0nul".to_owned(),
            "x".repeat(MAX_BYTES + 1),
            "\n".repeat(MAX_BYTES / 2),
        ] {
            assert!(patch(&root, change("NO_PROXY", Some(&value))).is_err());
            assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
        }
        for path in ["", ".", "relative/bin"] {
            assert!(patch(&root, change("PATH", Some(path))).is_err());
            assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
        }
    }
    #[test]
    fn unsafe_staging_file_never_changes_previous_settings() {
        let (_temp, root) = fixture();
        patch(&root, change("NO_PROXY", Some("synthetic.invalid"))).unwrap();
        let before = std::fs::read(root.join(FILE)).unwrap();
        // The last successful replace consumed the staging file; a wrong-kind replacement is untrusted.
        let staging = root.join("owner-environment.next.json");
        std::fs::create_dir(&staging).unwrap();
        assert_eq!(
            patch(&root, change("NO_PROXY", Some("changed.invalid")))
                .unwrap_err()
                .to_string(),
            "OWNER_SETTINGS_UNTRUSTED"
        );
        assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
        assert!(staging.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_are_case_insensitive_but_ambiguous_patches_are_rejected() {
        let (_temp, root) = fixture();
        patch(&root, change("https_proxy", Some("synthetic-proxy"))).unwrap();
        assert_eq!(status(&root).unwrap().overridden_names, ["HTTPS_PROXY"]);
        let before = std::fs::read(root.join(FILE)).unwrap();
        let changes = [
            ("HTTPS_PROXY".into(), Some("one".into())),
            ("https_proxy".into(), None),
        ]
        .into();
        assert_eq!(
            patch(&root, changes).unwrap_err().to_string(),
            "OWNER_SETTINGS_INVALID"
        );
        assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
        patch(&root, change("https_proxy", None)).unwrap();
        assert!(status(&root).unwrap().overridden_names.is_empty());
    }

    #[test]
    fn invalid_private_records_and_unsafe_path_kinds_fail_closed_without_repair() {
        let (_temp, root) = fixture();
        let path = root.join(FILE);
        drop(private::file(&path).unwrap());
        for bad in [
            b"{invalid synthetic JSON}".to_vec(),
            br#"{"version":2,"overrides":{}}"#.to_vec(),
            br#"{"version":1,"overrides":{},"extra":true}"#.to_vec(),
            br#"{"version":1,"overrides":{"NODE_OPTIONS":"synthetic-secret"}}"#.to_vec(),
            vec![b' '; MAX_BYTES + 1],
        ] {
            std::fs::write(&path, &bad).unwrap();
            let failure = status(&root).unwrap_err();
            assert!(!failure.to_string().contains("synthetic-secret"));
            assert!(environment(&root).is_err());
            assert!(patch(&root, BTreeMap::new()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bad);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(status(&root).is_err());
        std::fs::remove_dir(&path).unwrap();
        let target = root.join("target");
        drop(private::file(&target).unwrap());
        std::fs::write(&target, br#"{"version":1,"overrides":{}}"#).unwrap();
        std::fs::hard_link(&target, &path).unwrap();
        assert!(status(&root).is_err());
        std::fs::remove_file(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            symlink(&target, &path).unwrap();
            assert!(status(&root).is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&target, &path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(status(&root).is_err());
            assert!(patch(&root, BTreeMap::new()).is_err());
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
    }
}
