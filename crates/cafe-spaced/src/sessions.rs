//! Durable session state; callers hold the Host mutex. No process or prompt is replayed here.
use cafe_space_protocol::*;
use rusqlite::{Connection, OptionalExtension};
use std::io;

const MAX_SESSIONS: usize = 1_000;

#[derive(Debug)]
pub(crate) enum Error {
    Rejected(&'static str),
    Storage(io::Error),
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage(io::Error::other("SESSION_STORAGE_FAILED"))
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Storage(io::Error::other("SESSION_RECORD_INVALID"))
    }
}

fn validate(session: &SessionSnapshot) -> Result<()> {
    let event = ServerMessage::Event(ServerEvent {
        message_type: EventMessageType::Event,
        protocol_version: PROTOCOL_VERSION,
        event_stream_id: "validation".into(),
        sequence: 1,
        event: Event::SessionChanged(SessionChangedData {
            session: session.clone(),
        }),
    });
    encode_server_message(&event).map_err(|_| Error::Rejected("INVALID_SESSION_STATE"))?;
    if (session.state == SessionState::Running && session.active_run_id.is_none())
        || (!matches!(
            session.state,
            SessionState::Running | SessionState::WaitingForLocalUi | SessionState::Stopping
        ) && session.active_run_id.is_some())
        || session.stream_id.is_none()
        || (session.state == SessionState::Stopped && session.stop_reason.is_none())
    {
        return Err(Error::Rejected("INVALID_SESSION_STATE"));
    }
    if let SessionState::Failed(failure) = &session.state
        && (failure.code.len() > 64
            || !failure
                .code
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b == b'_'))
    {
        return Err(Error::Rejected("INVALID_FAILURE_CODE"));
    }
    Ok(())
}

fn decode(id: &str, json: &str) -> Result<SessionSnapshot> {
    if json.len() > MAX_FRAME_BYTES {
        return Err(Error::Storage(io::Error::other("SESSION_RECORD_INVALID")));
    }
    let session: SessionSnapshot = serde_json::from_str(json)?;
    if session.id != id || validate(&session).is_err() {
        return Err(Error::Storage(io::Error::other("SESSION_RECORD_INVALID")));
    }
    Ok(session)
}

pub(crate) fn get(database: &Connection, id: &str) -> Result<Option<SessionSnapshot>> {
    let json: Option<String> = database
        .query_row("SELECT snapshot FROM sessions WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    json.map(|json| decode(id, &json)).transpose()
}

pub(crate) fn list(database: &Connection) -> Result<Vec<SessionSnapshot>> {
    let mut query =
        database.prepare("SELECT id, snapshot FROM sessions ORDER BY rowid LIMIT 1001")?;
    let mut rows = query.query([])?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next()? {
        if sessions.len() == MAX_SESSIONS {
            return Err(Error::Rejected("SESSION_LIMIT"));
        }
        sessions.push(decode(
            &row.get::<_, String>(0)?,
            &row.get::<_, String>(1)?,
        )?);
    }
    Ok(sessions)
}

pub(crate) fn create(
    database: &Connection,
    params: &CreateSessionParams,
    project: Option<&ProjectSummary>,
    runtime: Option<&RuntimeSummary>,
) -> Result<(SessionSnapshot, bool)> {
    encode_client_message(&ClientMessage::Request(ClientRequest {
        message_type: RequestMessageType::Request,
        protocol_version: PROTOCOL_VERSION,
        request_id: "validation".into(),
        request: Request::CreateSession(params.clone()),
    }))
    .map_err(|_| Error::Rejected("INVALID_REQUEST"))?;
    let existing: Option<String> = database
        .query_row(
            "SELECT create_params FROM sessions WHERE id=?1",
            [&params.session_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(original) = existing {
        let original: CreateSessionParams = serde_json::from_str(&original)?;
        if original != *params {
            return Err(Error::Rejected("ID_CONFLICT"));
        }
        // An identical retry reads the durable outcome; it never starts another agent.
        return Ok((
            get(database, &params.session_id)?.ok_or(Error::Rejected("SESSION_NOT_FOUND"))?,
            false,
        ));
    }
    let project = project
        .filter(|p| p.id == params.project_id)
        .ok_or(Error::Rejected("PROJECT_NOT_APPROVED"))?;
    if project.cwd_fence != params.expected_cwd_fence {
        return Err(Error::Rejected("STALE_CWD_FENCE"));
    }
    if !runtime.is_some_and(|r| r.id == params.runtime_id && r.available) {
        return Err(Error::Rejected("RUNTIME_UNAVAILABLE"));
    }
    let count: i64 = database.query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))?;
    if count >= MAX_SESSIONS as i64 {
        return Err(Error::Rejected("SESSION_LIMIT"));
    }
    let mode = match &params.start {
        SessionStart::New => SessionMode::New,
        SessionStart::Resume(resume) => {
            let source = get(database, &resume.source_session_id)?
                .ok_or(Error::Rejected("SESSION_NOT_FOUND"))?;
            if source.project_id != params.project_id
                || source.runtime_id != params.runtime_id
                || source.cwd_fence != params.expected_cwd_fence
                || !matches!(
                    source.state,
                    SessionState::Stopped | SessionState::Failed(_)
                )
            {
                return Err(Error::Rejected("RESUME_UNAVAILABLE"));
            }
            SessionMode::Resume
        }
        SessionStart::Attach(_) => return Err(Error::Rejected("ATTACH_UNAVAILABLE")),
    };
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| Error::Storage(io::Error::other("SESSION_ID_FAILED")))?;
    let session = SessionSnapshot {
        id: params.session_id.clone(),
        project_id: params.project_id.clone(),
        runtime_id: params.runtime_id.clone(),
        cwd_fence: params.expected_cwd_fence.clone(),
        model: None,
        thinking_level: None,
        capabilities: SessionCapabilities {
            model_control: false,
            thinking_control: false,
            delivery_control: false,
            session_control: true,
            approval_for_session: false,
        },
        mode,
        state: SessionState::Starting,
        stop_reason: None,
        active_run_id: None,
        stream_id: Some(random.iter().map(|b| format!("{b:02x}")).collect()),
        title: None,
    };
    validate(&session)?;
    database.execute(
        "INSERT INTO sessions (id,create_params,snapshot) VALUES (?1,?2,?3)",
        rusqlite::params![
            session.id,
            serde_json::to_string(params)?,
            serde_json::to_string(&session)?
        ],
    )?;
    Ok((session, true))
}

pub(crate) fn open(
    database: &Connection,
    id: &str,
    stream: &str,
    cwd: &str,
) -> Result<SessionSnapshot> {
    let session = get(database, id)?.ok_or(Error::Rejected("SESSION_NOT_FOUND"))?;
    if session.stream_id.as_deref() != Some(stream) {
        return Err(Error::Rejected("STALE_STREAM"));
    }
    if session.cwd_fence != cwd {
        return Err(Error::Rejected("STALE_CWD_FENCE"));
    }
    Ok(session)
}

pub(crate) fn close(
    database: &Connection,
    id: &str,
    stream: &str,
    cwd: &str,
) -> Result<(SessionSnapshot, bool)> {
    let session = open(database, id, stream, cwd)?;
    if matches!(
        session.state,
        SessionState::Stopping | SessionState::Stopped | SessionState::Failed(_)
    ) {
        return Ok((session, false));
    }
    let mut stopping = session.clone();
    stopping.state = SessionState::Stopping;
    transition(database, &session, stopping)
}

/// Only the host's process/adapter observer calls this; no wire method supplies a new state.
pub(crate) fn transition(
    database: &Connection,
    previous: &SessionSnapshot,
    next: SessionSnapshot,
) -> Result<(SessionSnapshot, bool)> {
    if get(database, &previous.id)?.as_ref() != Some(previous) {
        return Err(Error::Rejected("STALE_SESSION"));
    }
    if next.id != previous.id
        || next.project_id != previous.project_id
        || next.runtime_id != previous.runtime_id
        || next.cwd_fence != previous.cwd_fence
        || next.stream_id != previous.stream_id
        || next.mode != previous.mode
    {
        return Err(Error::Rejected("INVALID_SESSION_TRANSITION"));
    }
    validate(&next)?;
    if next == *previous {
        return Ok((next, false));
    }
    if next.stop_reason == Some(SessionStopReason::SessionClosed)
        && previous.state != SessionState::Stopping
    {
        return Err(Error::Rejected("INVALID_SESSION_TRANSITION"));
    }
    use SessionState::*;
    let allowed = match (&previous.state, &next.state) {
        (Stopped | Failed(_), _) => false,
        (Stopping, Stopped | Failed(_)) => true,
        (Stopping, _) => false,
        (_, Stopping | Stopped | Failed(_)) => true,
        (Starting, Ready | WaitingForLocalUi) => true,
        (Ready, Running | WaitingForLocalUi) => true,
        (Running, Ready | WaitingForLocalUi) => true,
        (WaitingForLocalUi, Ready | Running) => true,
        _ => false,
    };
    if !allowed {
        return Err(Error::Rejected("INVALID_SESSION_TRANSITION"));
    }
    if previous.active_run_id.is_some()
        && next.active_run_id.is_some()
        && next.active_run_id != previous.active_run_id
    {
        return Err(Error::Rejected("STALE_RUN"));
    }
    save(database, &next)?;
    Ok((next, true))
}

fn save(database: &Connection, session: &SessionSnapshot) -> Result<()> {
    validate(session)?;
    if database.execute(
        "UPDATE sessions SET snapshot=?1 WHERE id=?2",
        rusqlite::params![serde_json::to_string(session)?, session.id],
    )? != 1
    {
        return Err(Error::Rejected("SESSION_NOT_FOUND"));
    }
    Ok(())
}

pub(crate) fn recover(database: &mut Connection) -> Result<()> {
    let transaction = database.transaction()?;
    for mut session in list(&transaction)? {
        if !matches!(
            session.state,
            SessionState::Stopped | SessionState::Failed(_)
        ) {
            session.state = SessionState::Stopped;
            session.stop_reason = Some(SessionStopReason::DaemonRestarted);
            session.active_run_id = None;
            save(&transaction, &session)?;
        }
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn approved(database: &Connection, id: &str) -> SessionSnapshot {
        let params = CreateSessionParams {
            session_id: id.into(),
            project_id: "project-fixture".into(),
            runtime_id: "pi".into(),
            expected_cwd_fence: "cwd-fixture".into(),
            start: SessionStart::New,
        };
        create(
            database,
            &params,
            Some(&ProjectSummary {
                id: params.project_id.clone(),
                name: "Fixture".into(),
                cwd_fence: params.expected_cwd_fence.clone(),
            }),
            Some(&RuntimeSummary {
                id: "pi".into(),
                name: "Fixture".into(),
                version: None,
                available: true,
            }),
        )
        .unwrap()
        .0
    }
    fn database() -> Connection {
        let database = Connection::open_in_memory().unwrap();
        database
            .execute_batch(include_str!("../../../fixtures/migrations/v2.sql"))
            .unwrap();
        database
    }
    #[test]
    fn every_state_pair_matches_the_shared_transition_matrix() {
        let matrix: std::collections::BTreeMap<String, Vec<String>> = serde_json::from_slice(
            include_bytes!("../../../fixtures/v2/session-transitions.json"),
        )
        .unwrap();
        let state = |name: &str| -> SessionState {
            serde_json::from_value(if name == "failed" {
                serde_json::json!({"state":name,"data":{"code":"AGENT_CRASHED"}})
            } else {
                serde_json::json!({"state":name})
            })
            .unwrap()
        };
        for (from, allowed) in &matrix {
            for to in matrix.keys() {
                let database = database();
                let mut previous = approved(&database, "matrix-session");
                previous.state = state(from);
                previous.active_run_id = matches!(
                    previous.state,
                    SessionState::Running
                        | SessionState::WaitingForLocalUi
                        | SessionState::Stopping
                )
                .then(|| "run-fixture".into());
                previous.stop_reason = (previous.state == SessionState::Stopped)
                    .then_some(SessionStopReason::AgentExited);
                save(&database, &previous).unwrap();
                let mut next = previous.clone();
                next.state = state(to);
                next.active_run_id = matches!(
                    next.state,
                    SessionState::Running
                        | SessionState::WaitingForLocalUi
                        | SessionState::Stopping
                )
                .then(|| "run-fixture".into());
                next.stop_reason =
                    (next.state == SessionState::Stopped).then_some(SessionStopReason::AgentExited);
                let writes = database.total_changes();
                let result = transition(&database, &previous, next.clone());
                if from == to {
                    assert_eq!(result.unwrap(), (previous, false));
                    assert_eq!(database.total_changes(), writes);
                } else if allowed.contains(to) {
                    assert_eq!(result.unwrap(), (next, true), "{from}->{to}");
                } else {
                    assert!(
                        matches!(result, Err(Error::Rejected("INVALID_SESSION_TRANSITION"))),
                        "{from}->{to}"
                    );
                    assert_eq!(get(&database, "matrix-session").unwrap(), Some(previous));
                    assert_eq!(database.total_changes(), writes);
                }
            }
        }
    }

    #[test]
    fn shared_lifecycle_is_authoritative_and_terminal_states_cannot_restart() {
        let database = database();
        let frames: Vec<ServerMessage> = serde_json::from_slice(include_bytes!(
            "../../../fixtures/v2/session-lifecycle.json"
        ))
        .unwrap();
        for frame in frames {
            let ServerMessage::Event(ServerEvent {
                event: Event::SessionChanged(data),
                ..
            }) = frame
            else {
                panic!("session fixture")
            };
            let expected = data.session;
            if let Some(previous) = get(&database, &expected.id).unwrap() {
                let mut next = expected.clone();
                next.stream_id = previous.stream_id.clone();
                let changed = transition(&database, &previous, next.clone()).unwrap();
                assert_eq!(changed, (next, true));
            } else {
                assert_eq!(approved(&database, &expected.id).state, expected.state);
            }
        }
        for terminal in list(&database).unwrap() {
            assert!(matches!(
                terminal.state,
                SessionState::Stopped | SessionState::Failed(_)
            ));
            let mut illegal = terminal.clone();
            illegal.state = SessionState::Ready;
            illegal.stop_reason = None;
            assert!(matches!(
                transition(&database, &terminal, illegal),
                Err(Error::Rejected("INVALID_SESSION_TRANSITION"))
            ));
        }
    }
    #[test]
    fn duplicate_create_close_and_stale_fences_do_not_write() {
        let database = database();
        let first = approved(&database, "session-fixture");
        let params = CreateSessionParams {
            session_id: first.id.clone(),
            project_id: first.project_id.clone(),
            runtime_id: first.runtime_id.clone(),
            expected_cwd_fence: first.cwd_fence.clone(),
            start: SessionStart::New,
        };
        let writes = database.total_changes();
        assert_eq!(
            create(&database, &params, None, None).unwrap(),
            (first.clone(), false)
        );
        let mut conflict = params.clone();
        conflict.runtime_id = "codex".into();
        assert!(matches!(
            create(&database, &conflict, None, None),
            Err(Error::Rejected("ID_CONFLICT"))
        ));
        assert!(matches!(
            open(&database, &first.id, "old", &first.cwd_fence),
            Err(Error::Rejected("STALE_STREAM"))
        ));
        assert!(matches!(
            close(
                &database,
                &first.id,
                first.stream_id.as_deref().unwrap(),
                "old"
            ),
            Err(Error::Rejected("STALE_CWD_FENCE"))
        ));
        assert_eq!(database.total_changes(), writes);
        let stopping = close(
            &database,
            &first.id,
            first.stream_id.as_deref().unwrap(),
            &first.cwd_fence,
        )
        .unwrap();
        assert_eq!(stopping.0.state, SessionState::Stopping);
        assert!(stopping.1);
        assert!(
            !close(
                &database,
                &first.id,
                first.stream_id.as_deref().unwrap(),
                &first.cwd_fence
            )
            .unwrap()
            .1
        );
        assert_eq!(list(&database).unwrap().len(), 1);
    }
    #[test]
    fn invalid_observations_never_modify_the_committed_record() {
        let database = database();
        let starting = approved(&database, "guarded");
        let writes = database.total_changes();
        let mut running = starting.clone();
        running.state = SessionState::Running;
        running.active_run_id = Some("run".into());
        assert!(matches!(
            transition(&database, &starting, running),
            Err(Error::Rejected("INVALID_SESSION_TRANSITION"))
        ));
        let mut renamed = starting.clone();
        renamed.project_id = "another-project".into();
        renamed.state = SessionState::Ready;
        assert!(matches!(
            transition(&database, &starting, renamed),
            Err(Error::Rejected("INVALID_SESSION_TRANSITION"))
        ));
        let mut private_failure = starting.clone();
        private_failure.state = SessionState::Failed(FailureState {
            code: "/private/file: details".into(),
        });
        assert!(matches!(
            transition(&database, &starting, private_failure),
            Err(Error::Rejected("INVALID_FAILURE_CODE"))
        ));
        assert_eq!(database.total_changes(), writes);
        assert_eq!(get(&database, "guarded").unwrap(), Some(starting));
    }

    #[test]
    fn absent_approval_or_runtime_never_persists_a_session() {
        let database = database();
        let params = CreateSessionParams {
            session_id: "s".into(),
            project_id: "p".into(),
            runtime_id: "pi".into(),
            expected_cwd_fence: "c".into(),
            start: SessionStart::New,
        };
        assert!(matches!(
            create(&database, &params, None, None),
            Err(Error::Rejected("PROJECT_NOT_APPROVED"))
        ));
        let project = ProjectSummary {
            id: "p".into(),
            name: "P".into(),
            cwd_fence: "c".into(),
        };
        assert!(matches!(
            create(&database, &params, Some(&project), None),
            Err(Error::Rejected("RUNTIME_UNAVAILABLE"))
        ));
        assert!(list(&database).unwrap().is_empty());
    }
    #[test]
    fn restart_preserves_terminal_outcomes_and_never_replays_a_prompt() {
        let mut database = database();
        let mut original = Vec::new();
        for (id, state) in [
            ("active", SessionState::Ready),
            (
                "failed",
                SessionState::Failed(FailureState {
                    code: "AGENT_CRASHED".into(),
                }),
            ),
            ("stopped", SessionState::Stopped),
        ] {
            let mut previous = approved(&database, id);
            if state == SessionState::Stopped {
                previous = close(
                    &database,
                    id,
                    previous.stream_id.as_deref().unwrap(),
                    &previous.cwd_fence,
                )
                .unwrap()
                .0;
            }
            let mut next = previous.clone();
            next.state = state;
            if next.state == SessionState::Stopped {
                next.stop_reason = Some(SessionStopReason::SessionClosed);
            }
            original.push(transition(&database, &previous, next).unwrap().0);
        }
        recover(&mut database).unwrap();
        let recovered = list(&database).unwrap();
        assert_eq!(
            recovered[0].stop_reason,
            Some(SessionStopReason::DaemonRestarted)
        );
        assert_eq!(recovered[0].active_run_id, None);
        assert_eq!(recovered[1..], original[1..]);
        recover(&mut database).unwrap();
        assert_eq!(list(&database).unwrap(), recovered);
    }
}
