//! Integrity checks and durable handover records shared by the daemon and its native shell.

use crate::private;
use cafe_space_protocol::check_binary_hash;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MARKER_BYTES: u64 = 4096;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub fn binary_hash(path: &Path) -> io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BINARY_BYTES {
        return Err(invalid(
            "daemon binary must be a bounded nonempty regular file",
        ));
    }
    hash_reader(File::open(path)?)
}

fn hash_reader(mut input: impl Read) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    let mut length = 0_u64;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > MAX_BINARY_BYTES {
            return Err(invalid("daemon binary exceeds size limit"));
        }
        digest.update(&buffer[..count]);
    }
    if length == 0 {
        return Err(invalid("empty daemon binary"));
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Stage only under a held private bin directory, never overwrite the running binary.
/// The caller serializes staging and handover with the startup-operation lock.
pub fn stage_binary(source: &Path, target: &Path, expected_hash: &str) -> io::Result<()> {
    if target.extension() != Some(std::ffi::OsStr::new("next")) {
        return Err(invalid(
            "staging must use a .next file, never the installed binary",
        ));
    }
    check_binary_hash(expected_hash).map_err(|_| invalid("invalid SHA-256 digest"))?;
    if binary_hash(source)? != expected_hash {
        return Err(invalid("bundled daemon hash mismatch"));
    }
    let parent = target
        .parent()
        .ok_or_else(|| invalid("missing bin directory"))?;
    let _guard = private::directory(parent)?;
    let temporary = target.with_extension("writing");
    let mut output = private::file(&temporary)?;
    let copied = (|| {
        output.set_len(0)?;
        let mut input = File::open(source)?;
        let count = io::copy(
            &mut Read::by_ref(&mut input).take(MAX_BINARY_BYTES + 1),
            &mut output,
        )?;
        if count == 0 || count > MAX_BINARY_BYTES {
            return Err(invalid("invalid daemon binary size"));
        }
        output.sync_all()
    })();
    drop(output);
    let checked = copied.and_then(|()| {
        if binary_hash(&temporary)? != expected_hash {
            return Err(invalid("staged daemon hash mismatch"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o700))?;
            private::existing_file(&temporary)?.sync_all()?;
        }
        private::replace_durable(&temporary, target)
    });
    if checked.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    checked
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoverPhase {
    Draining,
    Swapping,
    Starting,
    Committed,
    RolledBack,
    NotApplied,
    RecoveryRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Handover {
    pub old_hash: String,
    pub target_hash: String,
    pub phase: HandoverPhase,
}

impl Handover {
    fn validate(&self) -> io::Result<()> {
        check_binary_hash(&self.old_hash).map_err(|_| invalid("invalid old binary hash"))?;
        check_binary_hash(&self.target_hash).map_err(|_| invalid("invalid target binary hash"))?;
        if self.old_hash == self.target_hash {
            return Err(invalid("handover hashes must differ"));
        }
        Ok(())
    }

    pub fn read(root: &Path) -> io::Result<Option<Self>> {
        match std::fs::symlink_metadata(root) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
        let _guard = private::directory(root)?;
        let path = root.join("handover.json");
        let file = match private::existing_file(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > MAX_MARKER_BYTES {
            return Err(invalid("handover marker exceeds size limit"));
        }
        let marker: Self = serde_json::from_reader(file.take(MAX_MARKER_BYTES + 1))
            .map_err(|_| invalid("invalid handover marker"))?;
        marker.validate()?;
        Ok(Some(marker))
    }

    pub fn write(&self, root: &Path) -> io::Result<()> {
        self.validate()?;
        let _guard = private::directory(root)?;
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let temporary = root.join(format!(
            ".handover-{}-{}.writing",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = private::file(&temporary)?;
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        let result = written
            .and_then(|()| private::replace_durable(&temporary, &root.join("handover.json")));
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_and_staging_preserve_existing_bytes_on_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("bundled");
        std::fs::write(&source, b"abc").unwrap();
        let hash = binary_hash(&source).unwrap();
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let bin = temp.path().join("private-bin");
        let _guard = private::directory(&bin).unwrap();
        assert!(stage_binary(&source, &bin.join("cafe-spaced"), &hash).is_err());
        let next = bin.join("cafe-spaced.next");
        stage_binary(&source, &next, &hash).unwrap();
        assert_eq!(std::fs::read(&next).unwrap(), b"abc");
        drop(private::existing_file(&next).unwrap());
        assert!(stage_binary(&source, &next, &"f".repeat(64)).is_err());
        assert_eq!(std::fs::read(&next).unwrap(), b"abc");
        std::fs::write(&source, []).unwrap();
        assert!(binary_hash(&source).is_err());
        assert!(stage_binary(&source, &next, &hash).is_err());
        assert_eq!(std::fs::read(&next).unwrap(), b"abc");
    }

    #[test]
    fn handover_marker_is_private_atomic_validated_and_replaceable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _guard = private::directory(&root).unwrap();
        assert_eq!(Handover::read(&root).unwrap(), None);
        let mut marker = Handover {
            old_hash: "a".repeat(64),
            target_hash: "b".repeat(64),
            phase: HandoverPhase::Draining,
        };
        marker.write(&root).unwrap();
        assert_eq!(Handover::read(&root).unwrap(), Some(marker.clone()));
        marker.phase = HandoverPhase::Committed;
        marker.write(&root).unwrap();
        assert_eq!(Handover::read(&root).unwrap(), Some(marker.clone()));
        marker.target_hash = marker.old_hash.clone();
        assert!(marker.write(&root).is_err());
        assert_eq!(
            Handover::read(&root).unwrap().unwrap().phase,
            HandoverPhase::Committed
        );
        let mut file = private::file(&root.join("handover.json")).unwrap();
        file.set_len(0).unwrap();
        file.write_all(b"{\"oldHash\":\"secret\"}").unwrap();
        drop(file);
        assert!(Handover::read(&root).is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }
}
