//! Explicit local-owner registration. Paths never enter runtime summaries or events.
use cafe_space_adapters::pi::install::{Installation, VERSION};
use cafe_space_ipc::private;
use cafe_space_protocol::{RuntimeSummary, ToolApproval};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const FILE: &str = "pi-runtime.json";
const MAX_BYTES: usize = 65536;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    version: u32,
    installation: Option<Paths>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Paths {
    node: PathBuf,
    package_root: PathBuf,
}
fn error(code: &'static str) -> io::Error {
    io::Error::other(code)
}
fn load(root: &Path) -> io::Result<Settings> {
    let _guard = private::existing_directory(root).map_err(|_| error("PI_RUNTIME_UNTRUSTED"))?;
    let file = match private::existing_file(&root.join(FILE)) {
        Ok(file) => file,
        Err(value) if value.kind() == io::ErrorKind::NotFound => {
            return Ok(Settings {
                version: 1,
                installation: None,
            });
        }
        Err(_) => return Err(error("PI_RUNTIME_UNTRUSTED")),
    };
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("PI_RUNTIME_READ_FAILED"))?;
    if bytes.len() > MAX_BYTES {
        return Err(error("PI_RUNTIME_INVALID"));
    }
    let settings: Settings =
        serde_json::from_slice(&bytes).map_err(|_| error("PI_RUNTIME_INVALID"))?;
    if settings.version != 1 {
        return Err(error("PI_RUNTIME_INVALID"));
    }
    Ok(settings)
}

pub(crate) fn installation(root: &Path) -> io::Result<Option<Installation>> {
    load(root)?
        .installation
        .map(|paths| {
            Installation::inspect(&paths.node, &paths.package_root)
                .map_err(|error| io::Error::other(error.0))
        })
        .transpose()
}

pub(crate) fn summary(available: bool) -> RuntimeSummary {
    RuntimeSummary {
        id: "pi".into(),
        name: "Pi".into(),
        version: Some(VERSION.into()),
        available,
        tool_approval: ToolApproval::NeverAsks,
    }
}

pub(crate) fn list(root: &Path) -> Vec<RuntimeSummary> {
    match installation(root) {
        Ok(None) => vec![],
        Ok(Some(_)) => vec![summary(crate::owner_settings::environment(root).is_ok())],
        // A broken registration must not hide unrelated projects or session snapshots.
        Err(_) => vec![summary(false)],
    }
}

/// Host holds its admission mutex and state lock. No process is executed by registration.
pub(crate) fn configure(root: &Path, paths: Option<(&Path, &Path)>) -> io::Result<()> {
    let _guard = private::existing_directory(root).map_err(|_| error("PI_RUNTIME_UNTRUSTED"))?;
    let mut settings = load(root)?;
    settings.installation = paths
        .map(|(node, package_root)| {
            let checked = Installation::inspect(node, package_root)
                .map_err(|error| io::Error::other(error.0))?;
            Ok::<_, io::Error>(Paths {
                node: checked.node,
                package_root: package_root
                    .canonicalize()
                    .map_err(|_| error("PI_PACKAGE_UNAVAILABLE"))?,
            })
        })
        .transpose()?;
    let bytes = serde_json::to_vec(&settings).map_err(|_| error("PI_RUNTIME_INVALID"))?;
    if bytes.len() > MAX_BYTES {
        return Err(error("PI_RUNTIME_INVALID"));
    }
    let temporary = root.join("pi-runtime.next.json");
    let mut file = private::file(&temporary).map_err(|_| error("PI_RUNTIME_UNTRUSTED"))?;
    let written = file
        .set_len(0)
        .and_then(|()| file.write_all(&bytes))
        .and_then(|()| file.sync_all());
    drop(file);
    written
        .and_then(|()| private::replace_durable(&temporary, &root.join(FILE)))
        .map_err(|_| error("PI_RUNTIME_WRITE_UNKNOWN"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_adapters::pi::install::{CLI, PACKAGE};

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        drop(private::directory(&root).unwrap());
        let package = temp.path().join("synthetic-package");
        std::fs::create_dir_all(package.join("dist/bundle")).unwrap();
        std::fs::write(package.join(CLI), "throw Error('must not execute');").unwrap();
        std::fs::write(
            package.join("package.json"),
            serde_json::to_vec(&serde_json::json!({
                "name":PACKAGE, "version":VERSION, "bin":{"pi":CLI}
            }))
            .unwrap(),
        )
        .unwrap();
        let node = std::env::current_exe().unwrap();
        (temp, root, node, package)
    }

    #[test]
    fn registration_is_passive_private_persistent_and_revalidated() {
        let (_temp, root, node, package) = fixture();
        assert!(list(&root).is_empty());
        assert!(!root.join(FILE).exists());
        configure(&root, Some((&node, &package))).unwrap();
        assert!(private::existing_file(&root.join(FILE)).is_ok());
        let installed = installation(&root).unwrap().unwrap();
        assert_eq!(installed.node, node.canonicalize().unwrap());
        assert_eq!(installed.cli, package.join(CLI).canonicalize().unwrap());
        let public = serde_json::to_string(&list(&root)).unwrap();
        assert!(!public.contains("synthetic-package"));
        assert!(!public.contains("package_root"));
        let before = std::fs::read(root.join(FILE)).unwrap();
        std::fs::write(package.join("package.json"), b"{}").unwrap();
        assert!(!list(&root)[0].available);
        assert!(installation(&root).is_err());
        assert!(configure(&root, Some((&node, &package))).is_err());
        assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
        configure(&root, None).unwrap();
        assert!(list(&root).is_empty());
        assert!(installation(&root).unwrap().is_none());
    }

    #[test]
    fn invalid_records_and_untrusted_files_are_not_repaired_or_disclosed() {
        let (_temp, root, node, package) = fixture();
        let path = root.join(FILE);
        drop(private::file(&path).unwrap());
        for bad in [
            b"{synthetic-private-error}".to_vec(),
            br#"{"version":2,"installation":null}"#.to_vec(),
            br#"{"version":1,"installation":null,"extra":true}"#.to_vec(),
            vec![b' '; MAX_BYTES + 1],
        ] {
            std::fs::write(&path, &bad).unwrap();
            assert!(!list(&root)[0].available);
            assert!(installation(&root).is_err());
            let result = configure(&root, Some((&node, &package))).unwrap_err();
            assert_eq!(result.to_string(), "PI_RUNTIME_INVALID");
            assert!(configure(&root, None).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bad);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(installation(&root).is_err());
        std::fs::remove_dir(&path).unwrap();
        let target = root.join("target");
        drop(private::file(&target).unwrap());
        std::fs::write(&target, br#"{"version":1,"installation":null}"#).unwrap();
        std::fs::hard_link(&target, &path).unwrap();
        assert!(configure(&root, None).is_err());
        std::fs::remove_file(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            symlink(&target, &path).unwrap();
            assert!(installation(&root).is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&target, &path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(configure(&root, None).is_err());
        }
    }

    #[test]
    fn failed_staging_preserves_the_previous_registration() {
        let (_temp, root, node, package) = fixture();
        configure(&root, Some((&node, &package))).unwrap();
        let before = std::fs::read(root.join(FILE)).unwrap();
        std::fs::create_dir(root.join("pi-runtime.next.json")).unwrap();
        assert_eq!(
            configure(&root, None).unwrap_err().to_string(),
            "PI_RUNTIME_UNTRUSTED"
        );
        assert_eq!(std::fs::read(root.join(FILE)).unwrap(), before);
    }
}
