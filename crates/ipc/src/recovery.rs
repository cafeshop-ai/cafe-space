//! Durable recovery latch shared by the daemon and local rollback controller.
use crate::{
    installation::{Handover, HandoverPhase, STATE_SCHEMA_VERSION},
    private,
};
use cafe_space_protocol::check_binary_hash;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub const LATCH_FILE: &str = "recovery.json";
const MAX_BYTES: u64 = 4096;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPhase {
    Failed,
    Restoring,
    Restored,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryRetry {
    pub old_hash: String,
    pub target_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryLatch {
    pub phase: RecoveryPhase,
    pub from_schema: u32,
    pub to_schema: u32,
    pub failed_hash: String,
    pub backup_file: Option<String>,
    pub restored_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RecoveryRetry>,
}

impl RecoveryLatch {
    fn validate(&self) -> io::Result<()> {
        let invalid = || io::Error::other("RECOVERY_LATCH_INVALID");
        check_binary_hash(&self.failed_hash).map_err(|_| invalid())?;
        if let Some(retry) = &self.retry {
            check_binary_hash(&retry.old_hash).map_err(|_| invalid())?;
            check_binary_hash(&retry.target_hash).map_err(|_| invalid())?;
            if self.phase != RecoveryPhase::Restored
                || retry.old_hash == retry.target_hash
                || retry.old_hash == self.failed_hash
            {
                return Err(invalid());
            }
        }
        if self.from_schema == 0
            || self.from_schema > self.to_schema
            || self.to_schema > STATE_SCHEMA_VERSION
        {
            return Err(invalid());
        }
        if let Some(name) = &self.backup_file {
            let nonce = name
                .strip_prefix(&format!("state.sqlite3.v{}.", self.from_schema))
                .and_then(|s| s.strip_suffix(".bak"));
            if !nonce.is_some_and(|s| {
                s.len() == 32
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Err(invalid());
            }
        }
        if let Some(hash) = &self.restored_hash {
            check_binary_hash(hash).map_err(|_| invalid())?;
        }
        if let Some(name) = &self.failed_state
            && !name.strip_prefix("state.sqlite3.failed.").is_some_and(|s| {
                s.len() == 32
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(invalid());
        }
        if self.phase != RecoveryPhase::Failed
            && (self.backup_file.is_none() || self.restored_hash.is_none())
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn read(root: &Path) -> io::Result<Option<Self>> {
        let _guard = match private::existing_directory(root) {
            Ok(guard) => guard,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let file = match private::existing_file(&root.join(LATCH_FILE)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() > MAX_BYTES {
            return Err(io::Error::other("RECOVERY_LATCH_INVALID"));
        }
        let value: Self = serde_json::from_reader(file.take(MAX_BYTES + 1))
            .map_err(|_| io::Error::other("RECOVERY_LATCH_INVALID"))?;
        value.validate()?;
        Ok(Some(value))
    }

    pub fn write(&self, root: &Path) -> io::Result<()> {
        self.validate()?;
        let _guard = private::directory(root)?;
        let temporary = root.join(format!(
            ".recovery-{}-{}.writing",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let mut file = private::file(&temporary)?;
        if file.metadata()?.len() != 0 {
            return Err(io::Error::other("RECOVERY_TEMP_EXISTS"));
        }
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        let result =
            written.and_then(|()| private::replace_durable(&temporary, &root.join(LATCH_FILE)));
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }

    pub fn allows_retry(&self, marker: &Handover) -> bool {
        self.phase == RecoveryPhase::Restored
            && self.retry.as_ref().is_some_and(|retry| {
                retry.old_hash == marker.old_hash && retry.target_hash == marker.target_hash
            })
    }

    /// Called only after the controller successfully prepares a fresh handover under its startup lease.
    pub fn authorize_retry(root: &Path, marker: &Handover) -> io::Result<()> {
        if let Some(mut latch) = Self::read(root)? {
            if latch.phase != RecoveryPhase::Restored
                || marker.phase != HandoverPhase::Draining
                || marker.migration.is_some()
                || marker.old_hash == latch.failed_hash
                || Handover::read(root)?.as_ref() != Some(marker)
            {
                return Err(io::Error::other("RECOVERY_RETRY_NOT_AVAILABLE"));
            }
            latch.retry = Some(RecoveryRetry {
                old_hash: marker.old_hash.clone(),
                target_hash: marker.target_hash.clone(),
            });
            latch.write(root)?;
        }
        Ok(())
    }

    /// Only the newly committed upgrade may retire the old failure latch.
    pub fn retire(root: &Path) -> io::Result<()> {
        if let Some(latch) = Self::read(root)? {
            let marker = Handover::read(root)?
                .ok_or_else(|| io::Error::other("RECOVERY_RETRY_NOT_AVAILABLE"))?;
            if marker.phase != HandoverPhase::Committed || !latch.allows_retry(&marker) {
                return Err(io::Error::other("RECOVERY_RETRY_NOT_AVAILABLE"));
            }
            let name = latch
                .failed_state
                .as_ref()
                .filter(|_| latch.phase == RecoveryPhase::Restored)
                .ok_or_else(|| io::Error::other("RESTORE_REQUIRED"))?;
            let archive = root.join(format!("{name}.recovery.json"));
            if archive.try_exists()? {
                return Err(io::Error::other("RECOVERY_FILE_EXISTS"));
            }
            private::replace_durable(&root.join(LATCH_FILE), &archive)?;
        }
        Ok(())
    }
}

/// Database files are not bounded by the executable-size limit.
pub fn state_hash(path: &Path) -> io::Result<String> {
    let mut file = private::existing_file(path)?;
    if file.metadata()?.len() == 0 {
        return Err(io::Error::other("RECOVERY_STATE_EMPTY"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 16 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latch_is_private_strict_and_durable_without_touching_state_or_backup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("private");
        let _guard = private::directory(&root).unwrap();
        let mut latch = RecoveryLatch {
            phase: RecoveryPhase::Failed,
            from_schema: 5,
            to_schema: 6,
            failed_hash: "b".repeat(64),
            backup_file: Some(format!("state.sqlite3.v5.{}.bak", "a".repeat(32))),
            restored_hash: None,
            failed_state: None,
            retry: None,
        };
        latch.write(&root).unwrap();
        assert_eq!(RecoveryLatch::read(&root).unwrap(), Some(latch.clone()));
        assert!(!root.join("state.sqlite3").exists());
        assert!(!root.join(latch.backup_file.as_ref().unwrap()).exists());
        let original = std::fs::read(root.join(LATCH_FILE)).unwrap();
        latch.backup_file = Some("../outside.bak".into());
        assert!(latch.write(&root).is_err());
        assert_eq!(std::fs::read(root.join(LATCH_FILE)).unwrap(), original);
        let mut file = private::file(&root.join(LATCH_FILE)).unwrap();
        file.set_len(0).unwrap();
        file.write_all(b"{\"phase\":\"failed\",\"unknown\":true}")
            .unwrap();
        drop(file);
        assert!(RecoveryLatch::read(&root).is_err());
    }
}
