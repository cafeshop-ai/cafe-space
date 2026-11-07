//! Passive detection only: never invokes Pi, reads credentials or installs packages.
use super::{Error, Result, framing::Records};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

pub const PACKAGE: &str = "@earendil-works/pi-coding-agent";
pub const VERSION: &str = "1.5.0";
pub const CLI: &str = "dist/bundle/cli.js";

/// Node's entry-point resolver cannot use Windows verbatim paths returned by canonicalize.
pub fn node_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        let mut result = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:\\", drive as char)),
                Prefix::VerbatimUNC(server, share) => {
                    let mut root = OsString::from("\\\\");
                    root.push(server);
                    root.push("\\");
                    root.push(share);
                    root.push("\\");
                    PathBuf::from(root)
                }
                _ => return path.to_owned(),
            },
            _ => return path.to_owned(),
        };
        for component in components {
            if component != Component::RootDir {
                result.push(component);
            }
        }
        result
    }
    #[cfg(not(windows))]
    path.to_owned()
}

pub struct Installation {
    pub node: PathBuf,
    pub cli: PathBuf,
}
impl Installation {
    pub fn inspect(node: &Path, package_root: &Path) -> Result<Self> {
        if !node.is_absolute() || !package_root.is_absolute() {
            return Err(Error("PI_INSTALLATION_INVALID"));
        }
        let node = node
            .canonicalize()
            .map_err(|_| Error("PI_NODE_UNAVAILABLE"))?;
        let metadata = node.metadata().map_err(|_| Error("PI_NODE_UNAVAILABLE"))?;
        if !metadata.is_file() {
            return Err(Error("PI_NODE_UNAVAILABLE"));
        }
        #[cfg(windows)]
        if !node
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            return Err(Error("PI_NODE_UNAVAILABLE"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(Error("PI_NODE_UNAVAILABLE"));
            }
        }
        let root = package_root
            .canonicalize()
            .map_err(|_| Error("PI_PACKAGE_UNAVAILABLE"))?;
        let manifest = root.join("package.json");
        if !manifest.is_file() {
            return Err(Error("PI_PACKAGE_UNAVAILABLE"));
        }
        let mut bytes = Vec::new();
        File::open(manifest)
            .map_err(|_| Error("PI_PACKAGE_UNAVAILABLE"))?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("PI_PACKAGE_INVALID"))?;
        if bytes.len() > 65536 {
            return Err(Error("PI_PACKAGE_INVALID"));
        }
        let package: Value =
            serde_json::from_slice(&bytes).map_err(|_| Error("PI_PACKAGE_INVALID"))?;
        if package["name"] != PACKAGE
            || package["version"] != VERSION
            || package["bin"]["pi"] != CLI
        {
            return Err(Error("PI_VERSION_UNSUPPORTED"));
        }
        let cli = root
            .join(CLI)
            .canonicalize()
            .map_err(|_| Error("PI_CLI_UNAVAILABLE"))?;
        if !cli.starts_with(&root) || !cli.is_file() {
            return Err(Error("PI_CLI_UNAVAILABLE"));
        }
        Ok(Self { node, cli })
    }

    /// Caller owns the private directory handle and approved cwd identity throughout spawn.
    pub fn arguments(
        &self,
        native_id: &str,
        session_dir: &Path,
        name: Option<&str>,
        resume: Option<(&Path, &Path)>,
    ) -> Result<Vec<OsString>> {
        if native_id.is_empty()
            || native_id.len() > 128
            || !native_id
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !native_id
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !native_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !session_dir.is_absolute()
            || !session_dir.is_dir()
            || name.is_some_and(|name| name.len() > 256 || name.contains('\0'))
        {
            return Err(Error("PI_LAUNCH_INVALID"));
        }
        let session_dir = session_dir
            .canonicalize()
            .map_err(|_| Error("PI_SESSION_DIRECTORY_INVALID"))?;
        let mut resume_path = None;
        if let Some((file, cwd)) = resume {
            if name.is_some() {
                return Err(Error("PI_LAUNCH_INVALID"));
            }
            let file = file
                .canonicalize()
                .map_err(|_| Error("PI_RESUME_UNAVAILABLE"))?;
            if file.parent() != Some(session_dir.as_path()) {
                return Err(Error("PI_RESUME_UNAVAILABLE"));
            }
            let mut reader = Records::new(BufReader::new(
                File::open(&file)
                    .map_err(|_| Error("PI_RESUME_UNAVAILABLE"))?
                    .take((super::framing::MAX_RECORD_BYTES + 1) as u64),
            ));
            let Some(super::framing::Record::Json(header)) = reader
                .next_record()
                .map_err(|_| Error("PI_RESUME_UNAVAILABLE"))?
            else {
                return Err(Error("PI_RESUME_UNAVAILABLE"));
            };
            let stored_cwd = header["cwd"]
                .as_str()
                .ok_or(Error("PI_RESUME_UNAVAILABLE"))?;
            if header["type"] != "session"
                || header["version"] != 3
                || header["id"] != native_id
                || Path::new(stored_cwd).canonicalize().ok()
                    != Some(
                        cwd.canonicalize()
                            .map_err(|_| Error("PI_RESUME_UNAVAILABLE"))?,
                    )
                || !file.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .ends_with(&format!("_{native_id}.jsonl"))
                })
            {
                return Err(Error("PI_RESUME_UNAVAILABLE"));
            }
            resume_path = Some(file);
        }
        let mut args = vec![node_path(&self.cli).into_os_string()];
        args.extend(
            [
                "--mode",
                "rpc",
                "--offline",
                "--no-extensions",
                "--no-approve",
                "--no-themes",
                "--session-dir",
            ]
            .map(OsString::from),
        );
        args.push(node_path(&session_dir).into_os_string());
        if let Some(file) = resume_path {
            args.extend([
                OsString::from("--session"),
                node_path(&file).into_os_string(),
            ]);
        } else {
            args.extend([OsString::from("--session-id"), native_id.into()]);
        }
        if let Some(name) = name {
            args.extend([OsString::from("--name"), name.into()]);
        }
        Ok(args)
    }
}
