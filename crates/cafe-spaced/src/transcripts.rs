//! Bounded normalized display cache; native agent history remains authoritative.
//! Writers hold a SQLite transaction and the Host lock. No commands are replayed.
use crate::sessions::{Error, Result};
use cafe_space_adapters::pi;
use cafe_space_protocol::*;
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::io;

const MAX_MESSAGES: usize = 128;
const MAX_BYTES: usize = 128 * 1024;

fn invalid() -> Error {
    Error::Storage(io::Error::other("TRANSCRIPT_RECORD_INVALID"))
}

fn validate(session_id: &str, message: &TranscriptMessage) -> Result<String> {
    let payload = serde_json::to_string(message).map_err(|_| invalid())?;
    if payload.len() > pi::MAX_MESSAGE_BYTES {
        return Err(invalid());
    }
    encode_server_message(&ServerMessage::Event(ServerEvent {
        message_type: EventMessageType::Event,
        protocol_version: PROTOCOL_VERSION,
        event_stream_id: "validation".into(),
        sequence: 1,
        event: Event::MessageChanged(MessageChangedData {
            session_id: session_id.into(),
            message: message.clone(),
        }),
    }))
    .map_err(|_| invalid())?;
    Ok(payload)
}

pub(crate) fn load(
    database: &Connection,
    session_id: &str,
) -> Result<(Vec<TranscriptMessage>, bool)> {
    let truncated: Option<bool> = database
        .query_row(
            "SELECT truncated FROM transcript_cache WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    let mut query = database.prepare(
        "SELECT CASE WHEN length(CAST(id AS BLOB))<=128 THEN id END,
                CASE WHEN length(CAST(payload AS BLOB))<=?2 THEN payload END
         FROM transcript_messages WHERE session_id=?1 ORDER BY rowid LIMIT 129",
    )?;
    let mut rows = query.query(rusqlite::params![session_id, pi::MAX_MESSAGE_BYTES as i64])?;
    let mut messages = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let id = row.get::<_, Option<String>>(0)?.ok_or_else(invalid)?;
        let payload = row.get::<_, Option<String>>(1)?.ok_or_else(invalid)?;
        bytes += payload.len();
        if messages.len() == MAX_MESSAGES || bytes > MAX_BYTES {
            return Err(invalid());
        }
        let message: TranscriptMessage = serde_json::from_str(&payload).map_err(|_| invalid())?;
        if message.id != id {
            return Err(invalid());
        }
        validate(session_id, &message)?;
        messages.push(message);
    }
    Ok((messages, truncated.unwrap_or(false)))
}

pub(crate) fn put(
    database: &Transaction<'_>,
    session_id: &str,
    message: &TranscriptMessage,
) -> Result<()> {
    let payload = validate(session_id, message)?;
    // An upsert preserves rowid, so streaming updates never reorder a message.
    database.execute(
        "INSERT INTO transcript_messages(session_id,id,payload) VALUES (?1,?2,?3)
         ON CONFLICT(session_id,id) DO UPDATE SET payload=excluded.payload",
        rusqlite::params![session_id, message.id, payload],
    )?;
    loop {
        let (count, bytes): (i64, i64) = database.query_row(
            "SELECT count(*),coalesce(sum(length(CAST(payload AS BLOB))),0)
             FROM transcript_messages WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count <= MAX_MESSAGES as i64 && bytes <= MAX_BYTES as i64 {
            break;
        }
        database.execute(
            "DELETE FROM transcript_messages WHERE rowid=(
                SELECT rowid FROM transcript_messages WHERE session_id=?1 ORDER BY rowid LIMIT 1)",
            [session_id],
        )?;
        mark_truncated(database, session_id)?;
    }
    Ok(())
}

pub(crate) fn mark_truncated(database: &Transaction<'_>, session_id: &str) -> Result<()> {
    database.execute(
        "INSERT INTO transcript_cache(session_id,truncated) VALUES (?1,1)
         ON CONFLICT(session_id) DO UPDATE SET truncated=1",
        [session_id],
    )?;
    Ok(())
}

pub(crate) fn interrupt(
    database: &Transaction<'_>,
    session_id: &str,
) -> Result<Vec<TranscriptMessage>> {
    let (messages, _) = load(database, session_id)?;
    let mut interrupted = Vec::new();
    for mut message in messages {
        if message.status == TranscriptStatus::Streaming {
            pi::incomplete_message(&mut message).map_err(|_| invalid())?;
            put(database, session_id, &message)?;
            interrupted.push(message);
        }
    }
    Ok(interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions;

    fn database() -> Connection {
        let database = Connection::open_in_memory().unwrap();
        database.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
            .unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v5.sql"))
            .unwrap();
        sessions::tests::approved(&database, "session-cache");
        database
    }
    fn message(id: &str, text: &str) -> TranscriptMessage {
        TranscriptMessage {
            id: id.into(),
            role: TranscriptRole::Assistant,
            status: TranscriptStatus::Streaming,
            parts: vec![MessagePart::Text(TextPart { text: text.into() })],
        }
    }
    fn save(database: &mut Connection, message: &TranscriptMessage) {
        let transaction = database.transaction().unwrap();
        put(&transaction, "session-cache", message).unwrap();
        transaction.commit().unwrap();
    }
    #[test]
    fn streaming_upserts_preserve_order_and_interruption_is_explicit() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/pi-rpc/host-storage.json"))
                .unwrap();
        assert_eq!(contract["cacheLimits"]["messages"], MAX_MESSAGES);
        assert_eq!(contract["cacheLimits"]["encodedBytes"], MAX_BYTES);
        let mut database = database();
        save(&mut database, &message("first", "茶"));
        save(&mut database, &message("second", "two"));
        let mut completed = message("first", "茶 done");
        completed.status = TranscriptStatus::Complete;
        save(&mut database, &completed);
        let transaction = database.transaction().unwrap();
        let interrupted = interrupt(&transaction, "session-cache").unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].id, "second");
        assert_eq!(interrupted[0].status, TranscriptStatus::Error);
        assert!(
            serde_json::to_string(&interrupted[0])
                .unwrap()
                .contains("outcome is not inferred")
        );
        transaction.commit().unwrap();
        let (messages, truncated) = load(&database, "session-cache").unwrap();
        assert!(!truncated);
        assert_eq!(messages, vec![completed, interrupted[0].clone()]);
    }
    #[test]
    fn cache_bounds_count_and_encoded_utf8_bytes_and_rolls_back() {
        let mut database = database();
        for index in 0..=MAX_MESSAGES {
            save(&mut database, &message(&format!("m-{index}"), "tiny"));
        }
        let (messages, truncated) = load(&database, "session-cache").unwrap();
        assert!(truncated);
        assert_eq!(messages.len(), MAX_MESSAGES);
        assert_eq!(messages[0].id, "m-1");
        for index in 0..4 {
            save(
                &mut database,
                &message(&format!("large-{index}"), &"茶\n".repeat(10_000)),
            );
        }
        let (before, _) = load(&database, "session-cache").unwrap();
        assert_eq!(before.len(), 2);
        {
            let transaction = database.transaction().unwrap();
            put(
                &transaction,
                "session-cache",
                &message("not-committed", "no replay"),
            )
            .unwrap();
        }
        assert_eq!(load(&database, "session-cache").unwrap().0, before);
        let transaction = database.transaction().unwrap();
        assert!(put(&transaction, "missing-session", &message("orphan", "no")).is_err());
        assert!(
            put(
                &transaction,
                "session-cache",
                &message("oversized", &"x".repeat(pi::MAX_MESSAGE_BYTES))
            )
            .is_err()
        );
    }
    #[test]
    fn interrupted_tools_keep_the_last_observed_execution_status_not_an_invented_outcome() {
        let mut database = database();
        let tool = TranscriptMessage {
            id: "tool-message".into(),
            role: TranscriptRole::Tool,
            status: TranscriptStatus::Streaming,
            parts: vec![MessagePart::ToolCall(ToolCallPart {
                id: "tool-call".into(),
                name: "synthetic".into(),
                arguments: "{}".into(),
                output: None,
                status: ToolStatus::Running,
            })],
        };
        save(&mut database, &tool);
        let transaction = database.transaction().unwrap();
        let changed = interrupt(&transaction, "session-cache").unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].status, TranscriptStatus::Error);
        assert_eq!(changed[0].parts[0], tool.parts[0]);
        assert!(
            serde_json::to_string(&changed[0])
                .unwrap()
                .contains("outcome is not inferred")
        );
        transaction.commit().unwrap();
    }

    #[test]
    fn corrupt_rows_fail_closed_without_returning_their_contents() {
        let mut database = database();
        save(&mut database, &message("original", "valid"));
        database
            .execute(
                "UPDATE transcript_messages SET payload=?1",
                ["private malformed transcript"],
            )
            .unwrap();
        let Error::Storage(error) = load(&database, "session-cache").unwrap_err() else {
            panic!("storage error")
        };
        assert_eq!(error.to_string(), "TRANSCRIPT_RECORD_INVALID");
    }
}
