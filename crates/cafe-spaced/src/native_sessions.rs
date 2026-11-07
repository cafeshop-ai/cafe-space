//! Private Pi identity registry. Native IDs never become Space session IDs or wire fields.
use crate::sessions::{self, Error, Result};
use cafe_space_ipc::private;
use cafe_space_protocol::{CreateSessionParams, SessionStart, SessionState};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

fn valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}
fn invalid() -> Error {
    Error::Storage(io::Error::other("NATIVE_SESSION_RECORD_INVALID"))
}

pub(crate) fn get(database: &Connection, session_id: &str) -> Result<Option<String>> {
    let id: Option<Option<String>> = database
        .query_row(
            "SELECT CASE WHEN length(CAST(native_id AS BLOB))<=128 THEN native_id END
         FROM native_sessions WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    match id {
        None => Ok(None),
        Some(Some(id)) if valid(&id) => Ok(Some(id)),
        _ => Err(invalid()),
    }
}

pub(crate) fn record(
    database: &Transaction<'_>,
    params: &CreateSessionParams,
    native_id: &str,
) -> Result<()> {
    if !valid(native_id) {
        return Err(invalid());
    }
    match &params.start {
        SessionStart::New => {}
        SessionStart::Resume(source)
            if get(database, &source.source_session_id)?.as_deref() == Some(native_id) => {}
        _ => return Err(Error::Rejected("RESUME_UNAVAILABLE")),
    }
    let mut query =
        database.prepare("SELECT session_id FROM native_sessions WHERE native_id=?1 LIMIT 1001")?;
    let mut rows = query.query([native_id])?;
    while let Some(row) = rows.next()? {
        if matches!(params.start, SessionStart::New) {
            return Err(Error::Rejected("NATIVE_SESSION_ID_CONFLICT"));
        }
        let session = sessions::get(database, &row.get::<_, String>(0)?)?.ok_or_else(invalid)?;
        if !matches!(
            session.state,
            SessionState::Stopped | SessionState::Failed(_)
        ) {
            return Err(Error::Rejected("NATIVE_SESSION_IN_USE"));
        }
    }
    database.execute(
        "INSERT INTO native_sessions(session_id,native_id) VALUES (?1,?2)",
        rusqlite::params![params.session_id, native_id],
    )?;
    Ok(())
}

pub(crate) fn directory(root: &Path, native_id: &str) -> io::Result<(File, File)> {
    if !valid(native_id) {
        return Err(io::Error::other("NATIVE_SESSION_RECORD_INVALID"));
    }
    let parent = root.join("agent-sessions");
    let parent_guard = private::directory(&parent)?;
    // Lowercase IDs plus a fixed prefix avoid Windows device names and case aliases.
    let guard = private::directory(&parent.join(format!("pi-{native_id}")))?;
    Ok((parent_guard, guard))
}

/// Select one exact native file, never a newest/partial-ID guess. Directory guards stay held.
pub(crate) fn history(
    root: &Path,
    native_id: &str,
    directory: &File,
) -> io::Result<(PathBuf, File)> {
    let unavailable = || io::Error::other("PI_RESUME_UNAVAILABLE");
    if !valid(native_id) {
        return Err(unavailable());
    }
    let parent = root.join("agent-sessions").join(format!("pi-{native_id}"));
    let suffix = format!("_{native_id}.jsonl");
    let mut found = None;
    for (index, entry) in std::fs::read_dir(&parent)?.enumerate() {
        // shortcut: scan at most 1024 entries; store an exact locator if native directories grow.
        if index >= 1024 {
            return Err(unavailable());
        }
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(&suffix))
        {
            if found.is_some() {
                return Err(unavailable());
            }
            found = Some(entry.path());
        }
    }
    let path = found.ok_or_else(unavailable)?;
    #[cfg(windows)]
    let file = cafe_space_ipc::private::existing_file(&path).map_err(|_| unavailable())?;
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| unavailable())?;
        let metadata = file.metadata()?;
        // Pi creates 0644 files on Unix; held owner-only parents provide read privacy.
        if !metadata.is_file()
            || metadata.uid() != directory.metadata()?.uid()
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
        {
            return Err(unavailable());
        }
        file
    };
    #[cfg(windows)]
    let _ = directory;
    Ok((path, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_protocol::*;

    fn database() -> Connection {
        let database = Connection::open_in_memory().unwrap();
        database.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
            .unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v5.sql"))
            .unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v6.sql"))
            .unwrap();
        database
    }
    fn params(id: &str) -> CreateSessionParams {
        CreateSessionParams {
            session_id: id.into(),
            project_id: "project-fixture".into(),
            runtime_id: "pi".into(),
            expected_cwd_fence: "cwd-fixture".into(),
            start: SessionStart::New,
        }
    }
    #[test]
    fn history_requires_one_regular_owned_unaliased_exact_match() {
        let temp = tempfile::tempdir().unwrap();
        let guards = directory(temp.path(), "native-one").unwrap();
        let folder = temp.path().join("agent-sessions/pi-native-one");
        assert!(history(temp.path(), "native-one", &guards.1).is_err());
        assert!(history(temp.path(), "../outside", &guards.1).is_err());
        let path = folder.join("timestamp_native-one.jsonl");
        drop(private::file(&path).unwrap());
        drop(private::file(&folder.join("timestamp_native-one-extra.jsonl")).unwrap());
        assert_eq!(
            history(temp.path(), "native-one", &guards.1).unwrap().0,
            path
        );
        let duplicate = folder.join("other_native-one.jsonl");
        drop(private::file(&duplicate).unwrap());
        assert!(history(temp.path(), "native-one", &guards.1).is_err());
        std::fs::remove_file(&duplicate).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(history(temp.path(), "native-one", &guards.1).is_err());
        std::fs::remove_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let target = folder.join("target");
            drop(private::file(&target).unwrap());
            symlink(&target, &path).unwrap();
            assert!(history(temp.path(), "native-one", &guards.1).is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::hard_link(&target, &path).unwrap();
            assert!(history(temp.path(), "native-one", &guards.1).is_err());
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&target, &path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(history(temp.path(), "native-one", &guards.1).is_ok());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
            assert!(history(temp.path(), "native-one", &guards.1).is_err());
        }
    }

    #[test]
    fn history_scan_refuses_oversized_directories_instead_of_guessing() {
        let temp = tempfile::tempdir().unwrap();
        let guards = directory(temp.path(), "native-one").unwrap();
        let folder = temp.path().join("agent-sessions/pi-native-one");
        drop(private::file(&folder.join("timestamp_native-one.jsonl")).unwrap());
        for index in 0..1024 {
            std::fs::write(folder.join(format!("ignored-{index}")), b"").unwrap();
        }
        assert!(history(temp.path(), "native-one", &guards.1).is_err());
    }

    #[test]
    fn private_identity_is_transactional_and_not_part_of_public_snapshot() {
        let mut database = database();
        let transaction = database.transaction().unwrap();
        let session = sessions::tests::approved(&transaction, "space-id");
        record(&transaction, &params("space-id"), "private-native-identity").unwrap();
        assert_eq!(
            get(&transaction, "space-id").unwrap().as_deref(),
            Some("private-native-identity")
        );
        assert!(
            !serde_json::to_string(&session)
                .unwrap()
                .contains("private-native-identity")
        );
        drop(transaction);
        assert!(sessions::get(&database, "space-id").unwrap().is_none());
        assert!(get(&database, "space-id").unwrap().is_none());
    }
    #[test]
    fn resume_requires_exact_private_identity_and_exclusive_conversation() {
        let mut database = database();
        let transaction = database.transaction().unwrap();
        let source = sessions::tests::approved(&transaction, "source");
        record(&transaction, &params("source"), "native-one").unwrap();
        sessions::tests::approved(&transaction, "resumed");
        let mut resume = params("resumed");
        resume.start = SessionStart::Resume(ResumeStartParams {
            source_session_id: "source".into(),
        });
        assert!(matches!(
            record(&transaction, &resume, "other-native"),
            Err(Error::Rejected("RESUME_UNAVAILABLE"))
        ));
        assert!(matches!(
            record(&transaction, &resume, "native-one"),
            Err(Error::Rejected("NATIVE_SESSION_IN_USE"))
        ));
        let mut stopped = source.clone();
        stopped.state = SessionState::Stopped;
        stopped.stop_reason = Some(SessionStopReason::AgentExited);
        sessions::transition(&transaction, &source, stopped).unwrap();
        record(&transaction, &resume, "native-one").unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            get(&database, "resumed").unwrap().as_deref(),
            Some("native-one")
        );
    }
    #[test]
    fn invalid_or_missing_identities_never_fall_back_to_new() {
        let mut database = database();
        let transaction = database.transaction().unwrap();
        sessions::tests::approved(&transaction, "target");
        for id in [
            "", ".", "..", "../other", "a/b", "a\\b", "a\0b", "CON", "A", "a~1", "茶",
        ] {
            assert!(record(&transaction, &params("target"), id).is_err());
        }
        let mut resume = params("target");
        resume.start = SessionStart::Resume(ResumeStartParams {
            source_session_id: "missing".into(),
        });
        assert!(matches!(
            record(&transaction, &resume, "native-one"),
            Err(Error::Rejected("RESUME_UNAVAILABLE"))
        ));
        transaction
            .execute(
                "INSERT INTO native_sessions VALUES ('target','../bad-private-id')",
                [],
            )
            .unwrap();
        assert!(get(&transaction, "target").is_err());
        let root = tempfile::tempdir().unwrap();
        assert!(directory(root.path(), "../outside").is_err());
        assert!(!root.path().join("agent-sessions").exists());
    }
}
