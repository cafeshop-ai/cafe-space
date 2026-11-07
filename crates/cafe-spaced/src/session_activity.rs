//! Durable metadata and observation receipts, independent of the evictable display cache.
use crate::{
    sessions::{Error, Result},
    transcripts,
};
use cafe_space_adapters::pi::UserPreview;
use cafe_space_protocol::{TranscriptMessage, TranscriptRole, session_activity as rules};
use rusqlite::{Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::{
    io,
    time::{SystemTime, UNIX_EPOCH},
};

const SAVE_METADATA: &str = "INSERT INTO session_activity
    (session_id,last_activity_at,preview,last_user_message_id,last_user_message_order)
    VALUES (?1,?2,?3,?4,?5)
    ON CONFLICT(session_id) DO UPDATE SET last_activity_at=excluded.last_activity_at,
    preview=excluded.preview,last_user_message_id=excluded.last_user_message_id,
    last_user_message_order=excluded.last_user_message_order";
const SAVE_RECEIPT: &str = "INSERT INTO session_message_receipts
    (session_id,message_id,display_digest,user_message_order,preview_digest)
    VALUES (?1,?2,?3,?4,?5)
    ON CONFLICT(session_id,message_id) DO UPDATE SET
    display_digest=coalesce(excluded.display_digest,session_message_receipts.display_digest),
    user_message_order=coalesce(excluded.user_message_order,session_message_receipts.user_message_order),
    preview_digest=coalesce(excluded.preview_digest,session_message_receipts.preview_digest)
    WHERE (excluded.display_digest IS NOT NULL AND excluded.display_digest IS NOT session_message_receipts.display_digest)
       OR (excluded.preview_digest IS NOT NULL AND excluded.preview_digest IS NOT session_message_receipts.preview_digest)
       OR (excluded.user_message_order IS NOT NULL AND excluded.user_message_order IS NOT session_message_receipts.user_message_order)";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub last_activity_at: Option<u64>,
    pub user_preview: Option<UserPreview>,
}

fn invalid() -> Error {
    Error::Storage(io::Error::other("SESSION_ACTIVITY_INVALID"))
}

pub(crate) fn now() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .filter(|value| rules::validate_activity(*value).is_ok())
}

pub(crate) fn load(database: &Connection, session_id: &str) -> Result<Metadata> {
    let row = database
        .query_row(
            "SELECT last_activity_at,preview,last_user_message_id,last_user_message_order
         FROM session_activity WHERE session_id=?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((last_activity_at, text, id, order)) = row else {
        return Ok(Metadata::default());
    };
    let last_activity_at = last_activity_at
        .map(u64::try_from)
        .transpose()
        .map_err(|_| invalid())?;
    let order = order
        .map(u64::try_from)
        .transpose()
        .map_err(|_| invalid())?;
    if let Some(value) = last_activity_at {
        rules::validate_activity(value).map_err(|_| invalid())?;
    }
    let user_preview = match (id, order) {
        (Some(message_id), Some(insertion_order)) => {
            let preview = UserPreview {
                message_id,
                insertion_order,
                text,
            };
            preview.validate().map_err(|_| invalid())?;
            Some(preview)
        }
        (None, None) if text.is_none() => None,
        _ => return Err(invalid()),
    };
    Ok(Metadata {
        last_activity_at,
        user_preview,
    })
}

pub(crate) fn update(
    database: &Connection,
    session_id: &str,
    observed_at: Option<u64>,
    preview: Option<&UserPreview>,
) -> Result<bool> {
    let previous = load(database, session_id)?;
    let mut next = previous.clone();
    next.last_activity_at =
        rules::advance_activity(previous.last_activity_at, observed_at).map_err(|_| invalid())?;
    if let Some(preview) = preview {
        preview
            .validate()
            .map_err(|_| Error::Rejected("PI_PREVIEW_INVALID"))?;
        if let Some(old) = &previous.user_preview {
            if (old.message_id == preview.message_id)
                != (old.insertion_order == preview.insertion_order)
            {
                return Err(Error::Rejected("PI_PREVIEW_INVALID"));
            }
            if preview.insertion_order >= old.insertion_order {
                next.user_preview = Some(preview.clone());
            }
        } else {
            next.user_preview = Some(preview.clone());
        }
    }
    if next == previous {
        return Ok(false);
    }
    let preview = next.user_preview.as_ref();
    database.execute(
        SAVE_METADATA,
        rusqlite::params![
            session_id,
            next.last_activity_at
                .map(i64::try_from)
                .transpose()
                .map_err(|_| invalid())?,
            preview.and_then(|p| p.text.as_deref()),
            preview.map(|p| p.message_id.as_str()),
            preview
                .map(|p| i64::try_from(p.insertion_order))
                .transpose()
                .map_err(|_| invalid())?,
        ],
    )?;
    Ok(true)
}

// A receipt retains only digests, not another unbounded copy of the native transcript.
fn receipt(
    database: &Transaction<'_>,
    session_id: &str,
    message_id: &str,
    display: Option<&[u8]>,
    preview: Option<&UserPreview>,
) -> Result<bool> {
    let previous = database
        .query_row(
            "SELECT display_digest,user_message_order,preview_digest FROM session_message_receipts
         WHERE session_id=?1 AND message_id=?2",
            [session_id, message_id],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )
        .optional()?;
    let preview_digest = preview
        .map(|preview| {
            serde_json::to_vec(&preview.text).map(|bytes| Sha256::digest(bytes).to_vec())
        })
        .transpose()?;
    let order = preview
        .map(|preview| i64::try_from(preview.insertion_order))
        .transpose()
        .map_err(|_| invalid())?;
    if let Some((old_display, old_order, old_preview)) = &previous {
        if (old_display.is_none() && old_preview.is_none())
            || old_display
                .as_ref()
                .is_some_and(|digest| digest.len() != 32)
            || old_preview
                .as_ref()
                .is_some_and(|digest| digest.len() != 32)
            || old_order.is_some() != old_preview.is_some()
            || old_order.is_some_and(|order| order <= 0)
        {
            return Err(invalid());
        }
        if old_order.is_some() && order.is_some() && *old_order != order {
            return Err(Error::Rejected("PI_PREVIEW_INVALID"));
        }
    }
    // A source-only history receipt has unknown display, so its first display is not proof of activity.
    let novel = previous
        .as_ref()
        .is_none_or(|(old_display, _, old_preview)| {
            display
                .zip(old_display.as_deref())
                .is_some_and(|(new, old)| new != old)
                || preview_digest
                    .as_ref()
                    .zip(old_preview.as_ref())
                    .is_some_and(|(new, old)| new != old)
        });
    database.execute(
        SAVE_RECEIPT,
        rusqlite::params![session_id, message_id, display, order, preview_digest],
    )?;
    Ok(novel)
}

pub(crate) fn observe_message(
    database: &Transaction<'_>,
    session_id: &str,
    message: &TranscriptMessage,
    preview: Option<&UserPreview>,
    observed_at: Option<u64>,
) -> Result<bool> {
    if let Some(preview) = preview {
        preview
            .validate()
            .map_err(|_| Error::Rejected("PI_PREVIEW_INVALID"))?;
        if message.id != preview.message_id || message.role != TranscriptRole::User {
            return Err(Error::Rejected("PI_PREVIEW_INVALID"));
        }
    }
    let payload = transcripts::validate(session_id, message)?;
    let digest = Sha256::digest(payload.as_bytes());
    let novel = receipt(
        database,
        session_id,
        &message.id,
        Some(digest.as_ref()),
        preview,
    )?;
    update(database, session_id, observed_at.filter(|_| novel), preview)
}

pub(crate) fn initialize_history(
    database: &Transaction<'_>,
    session_id: &str,
    messages: &[TranscriptMessage],
    preview: Option<&UserPreview>,
) -> Result<bool> {
    let previous = load(database, session_id)?;
    if let Some(preview) = preview {
        preview
            .validate()
            .map_err(|_| Error::Rejected("PI_PREVIEW_INVALID"))?;
    }
    for message in messages {
        observe_message(
            database,
            session_id,
            message,
            preview.filter(|preview| preview.message_id == message.id),
            None,
        )?;
    }
    if let Some(preview) = preview {
        // Native history may have evicted the latest user display before reaching the Host.
        receipt(
            database,
            session_id,
            &preview.message_id,
            None,
            Some(preview),
        )?;
    }
    update(database, session_id, None, preview)?;
    Ok(load(database, session_id)? != previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_protocol::{MessagePart, TextPart, TranscriptStatus};

    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        for sql in [
            include_str!("../../../fixtures/migrations/v2.sql"),
            include_str!("../../../fixtures/migrations/v5.sql"),
            include_str!("../../../fixtures/migrations/v6.sql"),
        ] {
            db.execute_batch(sql).unwrap();
        }
        crate::sessions::tests::approved(&db, "one");
        crate::sessions::tests::approved(&db, "resumed");
        // These tests start from legacy sessions with unknown activity.
        db.execute("DELETE FROM session_activity", []).unwrap();
        db
    }
    fn message(id: &str, text: &str) -> TranscriptMessage {
        TranscriptMessage {
            id: id.into(),
            role: TranscriptRole::User,
            status: TranscriptStatus::Complete,
            parts: vec![MessagePart::Text(TextPart { text: text.into() })],
        }
    }
    fn preview(id: &str, order: u64, text: Option<&str>) -> UserPreview {
        UserPreview {
            message_id: id.into(),
            insertion_order: order,
            text: text.map(str::to_owned),
        }
    }

    #[test]
    fn evicted_replay_keeps_time_but_distinct_updates_advance_it() {
        let mut db = database();
        let first = message("message-1", "first");
        let source = preview(&first.id, 1, Some("first"));
        let tx = db.transaction().unwrap();
        transcripts::put(&tx, "one", &first).unwrap();
        assert!(observe_message(&tx, "one", &first, Some(&source), Some(100)).unwrap());
        for i in 2..=130 {
            transcripts::put(&tx, "one", &message(&format!("message-{i}"), "later")).unwrap();
        }
        assert!(
            !transcripts::load(&tx, "one")
                .unwrap()
                .0
                .iter()
                .any(|m| m.id == first.id)
        );
        assert!(transcripts::put(&tx, "one", &first).unwrap());
        assert!(!observe_message(&tx, "one", &first, Some(&source), Some(200)).unwrap());
        assert_eq!(load(&tx, "one").unwrap().last_activity_at, Some(100));
        let changed = message(&first.id, "edited");
        let changed_source = preview(&first.id, 1, Some("edited"));
        transcripts::put(&tx, "one", &changed).unwrap();
        assert!(observe_message(&tx, "one", &changed, Some(&changed_source), Some(300)).unwrap());
        tx.commit().unwrap();
        assert_eq!(load(&db, "one").unwrap().last_activity_at, Some(300));
    }

    #[test]
    fn provenance_changes_without_display_change_and_unknown_does_not_clear_it() {
        let mut db = database();
        let tx = db.transaction().unwrap();
        let display = message("message-1", "[Image not displayed]");
        let opaque = preview(&display.id, 1, None);
        assert!(observe_message(&tx, "one", &display, Some(&opaque), Some(100)).unwrap());
        let literal = preview(&display.id, 1, Some("[Image not displayed]"));
        assert!(observe_message(&tx, "one", &display, Some(&literal), Some(200)).unwrap());
        let before = load(&tx, "one").unwrap();
        let writes = tx.total_changes();
        assert!(!observe_message(&tx, "one", &display, None, Some(300)).unwrap());
        assert_eq!(tx.total_changes(), writes);
        assert_eq!(load(&tx, "one").unwrap(), before);
        assert!(observe_message(&tx, "one", &display, Some(&opaque), Some(400)).unwrap());
        assert_eq!(load(&tx, "one").unwrap().user_preview, Some(opaque));
    }

    #[test]
    fn history_is_not_activity_and_ordinals_are_scoped_to_each_public_session() {
        let mut db = database();
        let tx = db.transaction().unwrap();
        let old = preview("message-99", 99, Some("history"));
        assert!(initialize_history(&tx, "one", &[], Some(&old)).unwrap());
        assert_eq!(load(&tx, "one").unwrap().last_activity_at, None);
        assert!(
            !observe_message(
                &tx,
                "one",
                &message("message-99", "history"),
                Some(&old),
                Some(100)
            )
            .unwrap()
        );
        let new = preview("message-1", 1, Some("resumed history"));
        initialize_history(&tx, "resumed", &[], Some(&new)).unwrap();
        assert_eq!(load(&tx, "resumed").unwrap().user_preview, Some(new));
        update(&tx, "one", None, Some(&preview("message-2", 2, None))).unwrap();
        assert_eq!(load(&tx, "one").unwrap().user_preview, Some(old));
        let clear = preview("message-100", 100, None);
        assert!(update(&tx, "one", None, Some(&clear)).unwrap());
        assert_eq!(load(&tx, "one").unwrap().user_preview, Some(clear));
    }

    #[test]
    fn learning_unknown_history_provenance_does_not_prove_new_activity() {
        let mut db = database();
        let tx = db.transaction().unwrap();
        let display = message("historical-user", "history");
        assert!(!initialize_history(&tx, "one", std::slice::from_ref(&display), None).unwrap());
        let source = preview(&display.id, 1, Some("history"));
        assert!(observe_message(&tx, "one", &display, Some(&source), Some(100)).unwrap());
        assert_eq!(
            load(&tx, "one").unwrap(),
            Metadata {
                last_activity_at: None,
                user_preview: Some(source),
            }
        );
        let changed = preview(&display.id, 1, None);
        assert!(observe_message(&tx, "one", &display, Some(&changed), Some(200)).unwrap());
        assert_eq!(load(&tx, "one").unwrap().last_activity_at, Some(200));
    }

    #[test]
    fn transaction_rollback_preserves_display_metadata_and_receipt() {
        let mut db = database();
        let value = message("message-1", "pending");
        let source = preview(&value.id, 1, Some("pending"));
        {
            let tx = db.transaction().unwrap();
            transcripts::put(&tx, "one", &value).unwrap();
            observe_message(&tx, "one", &value, Some(&source), Some(100)).unwrap();
        }
        assert_eq!(load(&db, "one").unwrap(), Metadata::default());
        assert!(transcripts::load(&db, "one").unwrap().0.is_empty());
        assert_eq!(
            db.query_row("SELECT count(*) FROM session_message_receipts", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        let tx = db.transaction().unwrap();
        assert!(observe_message(&tx, "one", &value, Some(&source), Some(200)).unwrap());
        assert!(!update(&tx, "one", Some(50), None).unwrap());
        assert!(!update(&tx, "one", Some(0), None).unwrap());
        assert!(!update(&tx, "one", Some(rules::MAX_ACTIVITY_MILLIS + 1), None).unwrap());
        assert_eq!(load(&tx, "one").unwrap().last_activity_at, Some(200));
    }

    #[test]
    fn sqlite_integer_bounds_round_trip_and_negative_values_fail_closed() {
        let mut db = database();
        let tx = db.transaction().unwrap();
        let value = message("message-max", "text");
        let source = preview(&value.id, rules::MAX_INSERTION_ORDER, Some("text"));
        assert!(
            observe_message(
                &tx,
                "one",
                &value,
                Some(&source),
                Some(rules::MAX_ACTIVITY_MILLIS)
            )
            .unwrap()
        );
        assert_eq!(
            load(&tx, "one").unwrap(),
            Metadata {
                last_activity_at: Some(rules::MAX_ACTIVITY_MILLIS),
                user_preview: Some(source.clone()),
            }
        );
        assert!(!observe_message(&tx, "one", &value, Some(&source), Some(100)).unwrap());
        tx.execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        tx.execute(
            "UPDATE session_message_receipts SET user_message_order=-1",
            [],
        )
        .unwrap();
        assert!(observe_message(&tx, "one", &value, Some(&source), Some(100)).is_err());
        tx.execute("UPDATE session_activity SET last_user_message_order=-1", [])
            .unwrap();
        assert!(load(&tx, "one").is_err());
        tx.execute(
            "UPDATE session_activity SET last_user_message_order=?1,last_activity_at=-1",
            [i64::MAX],
        )
        .unwrap();
        assert!(load(&tx, "one").is_err());
    }

    #[test]
    fn corrupt_receipts_fail_closed_instead_of_inventing_activity() {
        for mutation in [
            "display_digest=NULL,user_message_order=NULL,preview_digest=NULL",
            "display_digest=zeroblob(31)",
            "preview_digest=NULL",
        ] {
            let mut db = database();
            let tx = db.transaction().unwrap();
            let value = message("message-1", "text");
            let source = preview(&value.id, 1, Some("text"));
            observe_message(&tx, "one", &value, Some(&source), Some(100)).unwrap();
            let before = load(&tx, "one").unwrap();
            tx.execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            tx.execute_batch(&format!("UPDATE session_message_receipts SET {mutation}"))
                .unwrap();
            assert!(observe_message(&tx, "one", &value, Some(&source), Some(200)).is_err());
            assert_eq!(load(&tx, "one").unwrap(), before);
        }
    }

    #[test]
    fn invalid_provenance_and_stored_metadata_fail_closed() {
        let mut db = database();
        let value = message("message-1", "text");
        let tx = db.transaction().unwrap();
        let valid = preview(&value.id, 1, Some("text"));
        observe_message(&tx, "one", &value, Some(&valid), Some(100)).unwrap();
        let before = load(&tx, "one").unwrap();
        for invalid in [
            preview("wrong", 1, Some("text")),
            preview(&value.id, 2, Some("text")),
            preview(&value.id, 1, Some("bad\ntext")),
            preview(&value.id, 0, None),
        ] {
            assert!(observe_message(&tx, "one", &value, Some(&invalid), Some(200)).is_err());
            assert_eq!(load(&tx, "one").unwrap(), before);
        }
        tx.execute(
            "UPDATE session_activity SET preview='bad'||char(10)||'text' WHERE session_id='one'",
            [],
        )
        .unwrap();
        assert!(load(&tx, "one").is_err());
        assert!(update(&tx, "one", Some(300), None).is_err());
    }
}
