//! Passive local discovery. Commands and profile paths never enter shared runtime summaries.
use crate::managed::Environment;
use cafe_space_protocol::{PiRuntimeDiscovery, RuntimeSummary, ToolApproval};
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;

pub(crate) struct Discovered {
    pub pi: PiRuntimeDiscovery,
    pub agents: Vec<DetectedAgent>,
}

pub(crate) struct DetectedAgent {
    pub id: &'static str,
    pub name: &'static str,
}

impl DetectedAgent {
    pub fn summary(&self) -> RuntimeSummary {
        RuntimeSummary {
            id: self.id.into(),
            name: self.name.into(),
            version: None,
            // Detection is not readiness: ACP launch support follows in #112–#116.
            available: false,
            tool_approval: ToolApproval::Unknown,
        }
    }
}

pub(crate) fn environment_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os(name).map(PathBuf::from)?;
    is_local_path(&path).then_some(path)
}

pub(crate) fn is_local_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    }
    #[cfg(not(windows))]
    {
        true
    }
}

// shortcut: inspect at most 256 unique directories; raise only with a measured latency budget.
fn append_paths(paths: &mut Vec<PathBuf>, extra: impl IntoIterator<Item = PathBuf>) {
    for path in extra {
        if paths.len() >= 256 {
            break;
        }
        if is_local_path(&path)
            && !paths.iter().any(|existing| {
                crate::managed::compare_environment_names(existing.as_os_str(), path.as_os_str())
                    .is_eq()
            })
        {
            paths.push(path);
        }
    }
}

/// Discovery never changes the child's D20 environment or imports a login shell.
pub(crate) fn search_paths(environment: Option<&Environment>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    append_paths(
        &mut paths,
        environment.into_iter().flat_map(Environment::path_entries),
    );
    #[cfg(windows)]
    append_paths(&mut paths, windows::registry_paths());
    let home = environment_path(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home {
        append_paths(
            &mut paths,
            [".local/bin", ".bun/bin", ".volta/bin"].map(|part| home.join(part)),
        );
        #[cfg(not(windows))]
        append_paths(
            &mut paths,
            [".npm-global/bin", ".local/share/pnpm", "Library/pnpm"].map(|part| home.join(part)),
        );
    }
    for name in [
        "PNPM_HOME",
        "BUN_INSTALL",
        "VOLTA_HOME",
        "NPM_CONFIG_PREFIX",
        "npm_config_prefix",
    ] {
        if let Some(root) = environment_path(name) {
            let directory = match name {
                "BUN_INSTALL" | "VOLTA_HOME" => root.join("bin"),
                "NPM_CONFIG_PREFIX" | "npm_config_prefix" if !cfg!(windows) => root.join("bin"),
                _ => root,
            };
            append_paths(&mut paths, [directory]);
        }
    }
    #[cfg(windows)]
    {
        if let Some(root) = environment_path("APPDATA") {
            append_paths(&mut paths, [root.join("npm")]);
        }
        if let Some(root) = environment_path("LOCALAPPDATA") {
            append_paths(&mut paths, ["pnpm", "agy/bin"].map(|part| root.join(part)));
        }
    }
    paths
}

fn find_command(paths: &[PathBuf], command: &str) -> Option<PathBuf> {
    let extensions: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    paths
        .iter()
        .take(256)
        .filter(|path| is_local_path(path))
        .find_map(|directory| {
            extensions.iter().find_map(|extension| {
                let path = directory
                    .join(format!("{command}{extension}"))
                    .canonicalize()
                    .ok()?;
                if !is_local_path(&path) {
                    return None;
                }
                let metadata = path.metadata().ok()?;
                if !metadata.is_file() {
                    return None;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o111 == 0 {
                        return None;
                    }
                }
                Some(path)
            })
        })
}

fn profile_present(path: &Path) -> bool {
    if !is_local_path(path) {
        return false;
    }
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    if !is_local_path(&path) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.take(64).filter_map(Result::ok).any(|entry| {
        entry.file_name() != "skills"
            && entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_file())
    })
}

fn detect_agents(paths: &[PathBuf], home: Option<&Path>) -> Vec<DetectedAgent> {
    [
        ("claude", "Claude Code", ".claude"),
        ("codex", "Codex", ".codex"),
        ("pi", "Pi", ".pi"),
        ("agy", "Antigravity CLI", ".gemini/antigravity-cli"),
    ]
    .into_iter()
    .filter_map(|(id, name, profile)| {
        let command = find_command(paths, id);
        (command.is_some() || home.is_some_and(|home| profile_present(&home.join(profile))))
            .then_some(DetectedAgent { id, name })
    })
    .collect()
}

pub(crate) fn discover(root: &Path) -> Discovered {
    let environment = crate::owner_settings::environment(root).ok();
    let paths = search_paths(environment.as_ref());
    let home = environment_path(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    Discovered {
        pi: crate::pi_runtime::discover_with_paths(&paths),
        agents: detect_agents(&paths, home.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_passive_partial_and_does_not_expose_paths() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let command = bin.join(if cfg!(windows) {
            "claude.cmd"
        } else {
            "claude"
        });
        let marker = temp.path().join("executed");
        std::fs::write(
            &command,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::create_dir_all(temp.path().join(".codex/skills")).unwrap();
        std::fs::create_dir_all(temp.path().join(".gemini/antigravity-cli")).unwrap();
        std::fs::write(
            temp.path().join(".gemini/antigravity-cli/settings.json"),
            "not parsed",
        )
        .unwrap();
        let agents = detect_agents(&[temp.path().join("missing"), bin], Some(temp.path()));
        assert_eq!(
            agents.iter().map(|agent| agent.id).collect::<Vec<_>>(),
            ["claude", "agy"]
        );
        let public = serde_json::to_string(
            &agents
                .iter()
                .map(DetectedAgent::summary)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(!public.contains("bin") && !public.contains("settings.json"));
        assert!(agents.iter().all(|agent| !agent.summary().available));
        assert!(!marker.exists());
        std::fs::write(temp.path().join(".codex/config.toml"), "not parsed").unwrap();
        assert_eq!(detect_agents(&[], Some(temp.path())).len(), 2);
    }

    #[test]
    fn invalid_pi_does_not_hide_other_agents_or_leak_private_errors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp
            .path()
            .join(cafe_space_ipc::BuildFlavor::Dev.directory_name());
        let host = crate::host::Host::new(
            crate::state::State::open(&root, cafe_space_ipc::BuildFlavor::Dev).unwrap(),
            &root,
            "discovery-fixture".into(),
            "a".repeat(64),
        )
        .unwrap();
        let items = host
            .accept_agent_discovery(Discovered {
                pi: PiRuntimeDiscovery {
                    node: Some("relative-private-node".into()),
                    package_root: Some("relative-private-package".into()),
                },
                agents: vec![DetectedAgent {
                    id: "claude",
                    name: "Claude Code",
                }],
            })
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "claude");
        assert!(!items[0].available);
        assert!(!root.join("pi-runtime.json").exists());
        assert!(!serde_json::to_string(&items).unwrap().contains("private"));
        let log = std::fs::read_to_string(root.join("logs/daemon.log")).unwrap();
        assert!(log.contains("AGENT_AUTO_REGISTRATION_FAILED"));
        assert!(!log.contains("relative-private"));
        assert!(host.discover_agents(crate::update::Access::Remote).is_err());
    }

    #[test]
    fn path_budget_filters_bad_entries_and_deduplicates() {
        let temp = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        append_paths(
            &mut paths,
            [
                PathBuf::new(),
                PathBuf::from("relative"),
                temp.path().into(),
                temp.path().into(),
            ],
        );
        assert_eq!(paths, [temp.path()]);
        append_paths(
            &mut paths,
            (0..300).map(|index| temp.path().join(index.to_string())),
        );
        assert_eq!(paths.len(), 256);
        #[cfg(windows)]
        for path in [r"\\server\share\bin", r"\\.\pipe\bin", r"C:relative"] {
            assert!(!is_local_path(Path::new(path)));
        }
    }
}
