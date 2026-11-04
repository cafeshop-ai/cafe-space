//! Private Pi identity registry. Native IDs never become Space session IDs or wire fields.
use crate::sessions::{self, Error, Result};
use cafe_space_ipc::private;
use cafe_space_protocol::{CreateSessionParams, SessionStart, SessionState};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::{fs::File, io, path::Path};

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
