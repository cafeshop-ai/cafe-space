//! Local-owner Pi discovery and registration. Paths never enter runtime summaries or events.
use cafe_space_adapters::pi::install::{Installation, PACKAGE, VERSION};
use cafe_space_ipc::private;
use cafe_space_protocol::{PiRuntimeDiscovery, RuntimeSummary, ToolApproval};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::managed::Environment;

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

fn environment_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os(name).map(PathBuf::from)?;
    path.is_absolute().then_some(path)
}

fn is_local_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        )
    }
    #[cfg(not(windows))]
    {
        path.is_absolute()
    }
}

fn node_candidates(environment: Option<&Environment>) -> Vec<PathBuf> {
    let executable = if cfg!(windows) { "node.exe" } else { "node" };
    let mut candidates: Vec<_> = environment
        .into_iter()
        .flat_map(Environment::path_entries)
        .take(248)
        .map(|directory| directory.join(executable))
        .collect();
    #[cfg(windows)]
    {
        for name in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = environment_path(name) {
                candidates.push(root.join("nodejs/node.exe"));
            }
        }
        if let Some(root) = environment_path("NVM_SYMLINK") {
            candidates.push(root.join("node.exe"));
        }
        if let Some(local) = environment_path("LOCALAPPDATA") {
            candidates.push(local.join("Programs/nodejs/node.exe"));
        }
        if let Some(home) = environment_path("USERPROFILE") {
            candidates.push(home.join("scoop/apps/nodejs/current/node.exe"));
        }
    }
    #[cfg(not(windows))]
    {
        candidates.extend(
            [
                "/usr/local/bin/node",
                "/usr/bin/node",
                "/opt/homebrew/bin/node",
            ]
            .map(PathBuf::from),
        );
        if let Some(home) = environment_path("HOME") {
            candidates.extend(
                [
                    ".nvm/current/bin/node",
                    ".volta/bin/node",
                    ".asdf/shims/node",
                    ".local/share/mise/shims/node",
                    ".local/share/fnm/aliases/default/bin/node",
                ]
                .map(|relative| home.join(relative)),
            );
        }
    }
    candidates
}

fn global_package_root(prefix: PathBuf) -> PathBuf {
    #[cfg(windows)]
    let root = prefix.join("node_modules");
    #[cfg(not(windows))]
    let root = prefix.join("lib/node_modules");
    root.join(PACKAGE)
}

fn package_candidates(node: Option<PathBuf>, environment: Option<&Environment>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for name in ["NPM_CONFIG_PREFIX", "npm_config_prefix"] {
        if let Some(prefix) = environment_path(name) {
            candidates.push(global_package_root(prefix));
        }
    }
    if let Some(home) = environment_path("PNPM_HOME") {
        candidates.push(home.join("global/5/node_modules").join(PACKAGE));
    }
    #[cfg(windows)]
    {
        if let Some(appdata) = environment_path("APPDATA") {
            candidates.push(appdata.join("npm/node_modules").join(PACKAGE));
        }
        if let Some(local) = environment_path("LOCALAPPDATA") {
            candidates.push(local.join("pnpm/global/5/node_modules").join(PACKAGE));
            candidates.push(local.join("Programs/nodejs/node_modules").join(PACKAGE));
        }
        for name in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = environment_path(name) {
                candidates.push(root.join("nodejs/node_modules").join(PACKAGE));
            }
        }
    }
    #[cfg(not(windows))]
    {
        for root in [
            "/usr/local/lib/node_modules",
            "/usr/lib/node_modules",
            "/opt/homebrew/lib/node_modules",
        ] {
            candidates.push(PathBuf::from(root).join(PACKAGE));
        }
        if let Some(home) = environment_path("HOME") {
            candidates.push(home.join(".npm-global/lib/node_modules").join(PACKAGE));
            candidates.push(
                home.join(".local/share/pnpm/global/5/node_modules")
                    .join(PACKAGE),
            );
            candidates.push(
                home.join("Library/pnpm/global/5/node_modules")
                    .join(PACKAGE),
            );
        }
    }
    if let Some(environment) = environment {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        for directory in environment.path_entries().into_iter().take(240) {
            let executable = directory.join(npm);
            if executable.is_file() {
                #[cfg(windows)]
                candidates.push(directory.join("node_modules").join(PACKAGE));
                #[cfg(not(windows))]
                if let Some(prefix) = directory.parent() {
                    candidates.push(global_package_root(prefix.to_path_buf()));
                }
            }
        }
    }
    if let Some(node) = node.as_deref()
        && let Some(directory) = node.parent()
    {
        candidates.push(directory.join("node_modules").join(PACKAGE));
        if directory
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
            && let Some(prefix) = directory.parent()
        {
            candidates.push(global_package_root(prefix.to_path_buf()));
            candidates.push(prefix.join("node_modules").join(PACKAGE));
        }
    }
    candidates
}

fn display_path(path: &Path) -> Option<String> {
    let value = path.to_str()?;
    (!value.is_empty() && value.len() <= 65_536 && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

// shortcut: inspect at most 256 candidates per kind; raise only with a measured latency budget.
fn discover_from_candidates(
    nodes: impl IntoIterator<Item = PathBuf>,
    packages: impl FnOnce(Option<PathBuf>) -> Vec<PathBuf>,
) -> PiRuntimeDiscovery {
    let node = nodes.into_iter().take(256).find_map(|candidate| {
        if !is_local_path(&candidate) {
            return None;
        }
        let path = candidate.canonicalize().ok()?;
        if !is_local_path(&path) {
            return None;
        }
        let path = Installation::inspect_node(&path).ok()?;
        Some((path.clone(), display_path(&path)?))
    });
    let package_root = packages(node.as_ref().map(|(path, _)| path.clone()))
        .into_iter()
        .take(256)
        .find_map(|candidate| {
            if !is_local_path(&candidate) {
                return None;
            }
            let root = candidate.canonicalize().ok()?;
            if !is_local_path(&root) {
                return None;
            }
            let root = Installation::inspect_package(&root).ok()?.0;
            display_path(&root)
        });
    PiRuntimeDiscovery {
        node: node.map(|(_, path)| path),
        package_root,
    }
}

/// Finds bounded local candidates by inspecting files only; it never executes Node or Pi.
pub(crate) fn discover(root: &Path) -> PiRuntimeDiscovery {
    let environment = crate::owner_settings::environment(root).ok();
    discover_from_candidates(node_candidates(environment.as_ref()), |node| {
        package_candidates(node, environment.as_ref())
    })
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
    fn discovery_is_passive_bounded_and_keeps_valid_candidates_when_others_fail() {
        let (temp, root, _, package) = fixture();
        let node = temp
            .path()
            .join(if cfg!(windows) { "node.exe" } else { "node" });
        let marker = temp.path().join("must-not-execute");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(
                &node,
                format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
            )
            .unwrap();
            std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        #[cfg(windows)]
        std::fs::write(&node, b"synthetic executable; never execute").unwrap();
        let bad_package = temp.path().join("bad-package");
        std::fs::create_dir_all(&bad_package).unwrap();
        std::fs::write(bad_package.join("package.json"), b"{}").unwrap();
        let discovered =
            discover_from_candidates([temp.path().join("missing-node"), node.clone()], |_| {
                vec![
                    temp.path().join("missing-package"),
                    bad_package,
                    package.clone(),
                ]
            });
        assert_eq!(
            discovered.node.as_deref(),
            node.canonicalize().unwrap().to_str()
        );
        assert_eq!(
            discovered.package_root.as_deref(),
            package.canonicalize().unwrap().to_str()
        );
        assert!(!marker.exists());
        assert!(!root.join(FILE).exists());
    }

    #[test]
    fn path_candidates_include_node_and_npm_global_locations() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let node = bin.join(if cfg!(windows) { "node.exe" } else { "node" });
        let npm = bin.join(if cfg!(windows) { "npm.cmd" } else { "npm" });
        std::fs::write(&node, "synthetic executable; never execute").unwrap();
        std::fs::write(&npm, "synthetic npm; never execute").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&node, &npm] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let path = std::env::join_paths([&bin]).unwrap();
        let environment = Environment::for_owner([("PATH".into(), path)]).unwrap();
        assert_eq!(node_candidates(Some(&environment)).first(), Some(&node));
        let package_root = if cfg!(windows) {
            bin.join("node_modules").join(PACKAGE)
        } else {
            temp.path().join("lib/node_modules").join(PACKAGE)
        };
        assert!(package_candidates(Some(node), Some(&environment)).contains(&package_root));
    }

    #[test]
    fn discovery_stops_at_the_path_and_package_candidate_limits() {
        let (temp, _, node, package) = fixture();
        let missing = temp.path().join("missing-candidate");
        let node_limit = std::iter::repeat_n(missing.clone(), 256).chain([node.clone()]);
        let node_result = discover_from_candidates(node_limit, |_| vec![package.clone()]);
        assert_eq!(node_result.node, None);
        assert_eq!(
            node_result.package_root.as_deref(),
            package.canonicalize().unwrap().to_str()
        );

        let package_limit = std::iter::repeat_n(missing, 256).chain([package]);
        let package_result =
            discover_from_candidates(std::iter::empty(), |_| package_limit.collect());
        assert_eq!(package_result.node, None);
        assert_eq!(package_result.package_root, None);
    }

    #[cfg(windows)]
    #[test]
    fn discovery_skips_unc_candidates_before_inspecting_files() {
        let discovered =
            discover_from_candidates([PathBuf::from(r"\\server\share\node.exe")], |_| {
                vec![PathBuf::from(
                    r"\\server\share\node_modules\@earendil-works\pi-coding-agent",
                )]
            });
        assert_eq!(discovered.node, None);
        assert_eq!(discovered.package_root, None);
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
