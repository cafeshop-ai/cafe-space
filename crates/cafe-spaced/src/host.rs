//! Authoritative update gate, durable state and bounded event history for this daemon boot.

use crate::{
    sessions,
    state::State,
    update::{Access, Gate},
};
use cafe_space_ipc::installation::{Handover, HandoverPhase};
use cafe_space_protocol::*;
use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{Notify, broadcast};

const HISTORY: usize = 64;

pub struct Host {
    inner: Mutex<Inner>,
    next_owner: AtomicU64,
    pub shutdown: Notify,
}

struct Inner {
    state: State,
    gate: Gate,
    root: PathBuf,
    binary_hash: String,
    stream_id: String,
    sequence: u32,
    history: VecDeque<ServerEvent>,
    events: broadcast::Sender<ServerEvent>,
    provisional: bool,
    migration_failed: bool,
    normal_shutdown: bool,
}

pub struct Dispatch {
    pub result: RequestResult,
    pub subscription: Option<(broadcast::Receiver<ServerEvent>, Vec<ServerEvent>)>,
    pub exit_after_reply: bool,
}

impl Host {
    pub fn new(
        mut state: State,
        root: &Path,
        stream_id: String,
        binary_hash: String,
    ) -> io::Result<Self> {
        let mut gate = Gate::new(binary_hash.clone()).map_err(io::Error::other)?;
        let mut provisional = false;
        if let Some(mut marker) = Handover::read(root)? {
            match marker.phase {
                HandoverPhase::Committed
                | HandoverPhase::RolledBack
                | HandoverPhase::NotApplied => {}
                HandoverPhase::RecoveryRequired => {
                    return Err(invalid("handover requires owner recovery"));
                }
                _ if binary_hash == marker.old_hash => {
                    marker.phase = HandoverPhase::NotApplied;
                    marker.write(root)?;
                }
                _ if binary_hash == marker.target_hash => {
                    provisional = true;
                    gate.provisional(true);
                }
                _ => return Err(invalid("running binary does not match active handover")),
            }
        }
        if !provisional {
            crate::migrations::run(&mut state.database, root)?;
            sessions::recover(&mut state.database).map_err(session_io)?;
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                state,
                gate,
                root: root.to_owned(),
                binary_hash,
                stream_id,
                sequence: 0,
                history: VecDeque::new(),
                events: broadcast::channel(HISTORY).0,
                provisional,
                migration_failed: false,
                normal_shutdown: false,
            }),
            next_owner: AtomicU64::new(1),
            shutdown: Notify::new(),
        })
    }

    pub fn connection_id(&self) -> u64 {
        self.next_owner.fetch_add(1, Ordering::Relaxed)
    }

    pub fn log(&self, code: &str) -> io::Result<()> {
        self.lock()?.state.log(code)
    }

    pub fn dispatch(&self, access: Access, owner: u64, request: Request) -> io::Result<Dispatch> {
        let mut inner = self.lock()?;
        inner.refresh_commit()?;
        let mut subscription = None;
        let mut exit_after_reply = false;
        let result = match request {
            Request::GetStatus => applied(Response::Health(HealthResponse {
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                ready: !inner.provisional && !inner.normal_shutdown,
                update: inner.gate.state(),
                binary_hash: Some(inner.binary_hash.clone()),
                event_stream_id: Some(inner.stream_id.clone()),
                awaiting_update_confirmation: inner.provisional.then_some(true),
            })),
            _ if inner.normal_shutdown => rejected("SHUTDOWN_IN_PROGRESS"),
            Request::Shutdown => {
                if access != Access::Local {
                    rejected("LOCAL_ONLY")
                } else {
                    // shortcut: M0 has no agents; M1 must stop the real registry before checkpointing.
                    inner.state.checkpoint()?;
                    inner.state.log("DAEMON_STOP_REQUESTED")?;
                    inner.normal_shutdown = true;
                    exit_after_reply = true;
                    applied(Response::ShutdownAccepted)
                }
            }
            _ if inner.provisional => rejected("UPDATE_IN_PROGRESS"),
            Request::ListProjects => applied(Response::Projects(ProjectList { items: vec![] })),
            Request::ListRuntimes => applied(Response::Runtimes(RuntimeList { items: vec![] })),
            Request::ListSessions => applied(Response::Sessions(SessionList {
                items: sessions::list(&inner.state.database).map_err(session_io)?,
                event_stream_id: inner.stream_id.clone(),
                sequence: inner.sequence,
            })),
            Request::GetSession(params) => {
                match sessions::get(&inner.state.database, &params.session_id)
                    .map_err(session_io)?
                {
                    Some(session) => inner.session_result(Ok((session, false)))?,
                    None => rejected("SESSION_NOT_FOUND"),
                }
            }
            Request::SubscribeEvents(params) => {
                let oldest = inner
                    .history
                    .front()
                    .map_or(inner.sequence + 1, |event| event.sequence);
                let resumed = params.after.as_ref().is_some_and(|cursor| {
                    cursor.event_stream_id == inner.stream_id
                        && cursor.sequence <= inner.sequence
                        && cursor.sequence >= oldest - 1
                });
                let replay = if resumed {
                    let after = params
                        .after
                        .as_ref()
                        .expect("resumed cursor exists")
                        .sequence;
                    inner
                        .history
                        .iter()
                        .filter(|event| event.sequence > after)
                        .cloned()
                        .collect()
                } else {
                    vec![]
                };
                let sessions = if resumed {
                    vec![]
                } else {
                    sessions::list(&inner.state.database).map_err(session_io)?
                };
                subscription = Some((inner.events.subscribe(), replay));
                applied(Response::Subscription(SubscriptionResponse {
                    event_stream_id: inner.stream_id.clone(),
                    sequence: inner.sequence,
                    mode: if resumed {
                        SubscriptionMode::Resumed
                    } else {
                        SubscriptionMode::SnapshotRequired
                    },
                    snapshot: (!resumed).then(|| SnapshotResponse {
                        update: inner.gate.state(),
                        projects: vec![],
                        runtimes: vec![],
                        sessions,
                    }),
                }))
            }
            Request::PrepareUpdate(params) => {
                let previous = inner.gate.clone();
                match inner.gate.prepare(access, owner, params) {
                    Err(code) => rejected(code),
                    Ok(update) => {
                        exit_after_reply = self.commit_transition(&mut inner, previous)?;
                        applied(Response::Update(UpdateResponse { update }))
                    }
                }
            }
            Request::CancelUpdate => {
                let previous = inner.gate.state();
                match inner.gate.cancel(access, owner) {
                    Err(code) => rejected(code),
                    Ok(update) => {
                        if update != previous {
                            inner.publish()?;
                        }
                        applied(Response::Update(UpdateResponse { update }))
                    }
                }
            }
            _ if !inner.gate.admits_work() => rejected("UPDATE_IN_PROGRESS"),
            Request::CreateSession(_) | Request::OpenSession(_) | Request::CloseSession(_)
                if access != Access::Local =>
            {
                rejected("LOCAL_ONLY")
            }
            Request::CreateSession(params) => {
                inner.check_event_capacity()?;
                // M1-2 and adapters supply approved profiles; never create a synthetic production default.
                let result = sessions::create(&inner.state.database, &params, None, None);
                inner.session_result(result)?
            }
            Request::OpenSession(params) => {
                let result = sessions::open(
                    &inner.state.database,
                    &params.session_id,
                    &params.expected_stream_id,
                    &params.expected_cwd_fence,
                );
                inner.session_result(result.map(|session| (session, false)))?
            }
            Request::CloseSession(params) => {
                inner.check_event_capacity()?;
                let result = sessions::close(
                    &inner.state.database,
                    &params.session_id,
                    &params.expected_stream_id,
                    &params.expected_cwd_fence,
                );
                let stopping = result
                    .as_ref()
                    .ok()
                    .filter(|(session, _)| session.state == SessionState::Stopping)
                    .and_then(|(session, _)| session.stream_id.clone());
                let response = inner.session_result(result)?;
                if let Some(operation_id) = stopping {
                    RequestResult::Dispatched(DispatchedResult { operation_id })
                } else {
                    response
                }
            }
            _ => rejected("NOT_IMPLEMENTED"),
        };
        let result = match encode_server_message(&ServerMessage::Response(ServerResponse {
            message_type: ResponseMessageType::Response,
            protocol_version: PROTOCOL_VERSION,
            request_id: "r".repeat(128),
            result: result.clone(),
        })) {
            Ok(_) => result,
            Err(ProtocolError::FrameTooLarge) => {
                subscription = None;
                rejected("RESPONSE_TOO_LARGE")
            }
            Err(_) => return Err(invalid("invalid host response")),
        };
        Ok(Dispatch {
            result,
            subscription,
            exit_after_reply,
        })
    }

    pub fn normal_shutdown_requested(&self) -> io::Result<bool> {
        Ok(self.lock()?.normal_shutdown)
    }

    fn commit_transition(&self, inner: &mut Inner, previous: Gate) -> io::Result<bool> {
        let update = inner.gate.state();
        if update == previous.state() {
            return Ok(false);
        }
        let exit = update == UpdateState::Draining;
        if exit {
            let marker = Handover {
                old_hash: inner.binary_hash.clone(),
                target_hash: inner
                    .gate
                    .target_hash()
                    .expect("draining lease has a target")
                    .into(),
                phase: HandoverPhase::Draining,
                migration: None,
            };
            if let Err(error) = inner
                .state
                .checkpoint()
                .and_then(|()| marker.write(&inner.root))
            {
                match Handover::read(&inner.root) {
                    Ok(current) if current.as_ref() != Some(&marker) => inner.gate = previous,
                    _ => self.shutdown.notify_one(),
                }
                return Err(error);
            }
        }
        if let Err(error) = inner.publish() {
            if exit {
                self.shutdown.notify_one();
            }
            return Err(error);
        }
        Ok(exit)
    }

    #[cfg(test)]
    pub(crate) fn create_for_test(
        &self,
        params: &CreateSessionParams,
    ) -> io::Result<RequestResult> {
        let mut inner = self.lock()?;
        inner.check_event_capacity()?;
        let result = sessions::create(
            &inner.state.database,
            params,
            Some(&ProjectSummary {
                id: params.project_id.clone(),
                name: "Fixture".into(),
                cwd_fence: params.expected_cwd_fence.clone(),
            }),
            Some(&RuntimeSummary {
                id: params.runtime_id.clone(),
                name: "Synthetic".into(),
                version: None,
                available: true,
            }),
        );
        inner.session_result(result)
    }

    #[cfg(test)]
    pub(crate) fn set_active_for_test(&self, count: usize) -> io::Result<bool> {
        let mut inner = self.lock()?;
        let previous = inner.gate.clone();
        inner.gate.set_active_for_test(count);
        let exit = self.commit_transition(&mut inner, previous)?;
        if exit {
            self.shutdown.notify_one();
        }
        Ok(exit)
    }

    pub fn disconnected(&self, owner: u64) {
        if let Ok(mut inner) = self.inner.lock()
            && inner.gate.disconnect(owner)
            && inner.publish().is_err()
        {
            self.shutdown.notify_one();
        }
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| invalid("host state lock is poisoned"))
    }
}

impl Inner {
    fn session_result(
        &mut self,
        result: sessions::Result<(SessionSnapshot, bool)>,
    ) -> io::Result<RequestResult> {
        match result {
            Ok((session, changed)) => {
                if changed {
                    self.publish_event(Event::SessionChanged(SessionChangedData {
                        session: session.clone(),
                    }))?;
                }
                Ok(applied(Response::Session(Box::new(SessionResponse {
                    session: SessionView {
                        session,
                        messages: vec![],
                        history_truncated: false,
                        approvals: vec![],
                    },
                    event_stream_id: self.stream_id.clone(),
                    sequence: self.sequence,
                }))))
            }
            Err(sessions::Error::Rejected(code)) => Ok(rejected(code)),
            Err(error) => Err(session_io(error)),
        }
    }

    fn check_event_capacity(&self) -> io::Result<()> {
        if self.sequence >= MAX_EVENT_SEQUENCE {
            return Err(invalid("event sequence exhausted"));
        }
        Ok(())
    }

    fn publish(&mut self) -> io::Result<()> {
        self.publish_event(Event::UpdateChanged(UpdateResponse {
            update: self.gate.state(),
        }))
    }

    fn publish_event(&mut self, event: Event) -> io::Result<()> {
        self.check_event_capacity()?;
        let event = ServerEvent {
            message_type: EventMessageType::Event,
            protocol_version: PROTOCOL_VERSION,
            event_stream_id: self.stream_id.clone(),
            sequence: self.sequence + 1,
            event,
        };
        encode_server_message(&ServerMessage::Event(event.clone()))
            .map_err(|_| invalid("invalid host event"))?;
        self.sequence = event.sequence;
        self.history.push_back(event.clone());
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let _ = self.events.send(event);
        Ok(())
    }

    fn refresh_commit(&mut self) -> io::Result<()> {
        if self.migration_failed {
            return Err(invalid("SCHEMA_MIGRATION_FAILED"));
        }
        if self.provisional
            && Handover::read(&self.root)?.is_some_and(|marker| {
                marker.target_hash == self.binary_hash && marker.phase == HandoverPhase::Committed
            })
        {
            if crate::migrations::run(&mut self.state.database, &self.root)
                .and_then(|()| sessions::recover(&mut self.state.database).map_err(session_io))
                .is_err()
            {
                // Latch until restart: health polling must not create repeated recovery backups.
                self.migration_failed = true;
                self.state.log("SCHEMA_MIGRATION_FAILED")?;
                return Err(invalid("SCHEMA_MIGRATION_FAILED"));
            }
            self.gate.provisional(false);
            self.provisional = false;
            self.publish()?;
        }
        Ok(())
    }
}

fn session_io(error: sessions::Error) -> io::Error {
    match error {
        sessions::Error::Storage(error) => error,
        sessions::Error::Rejected(code) => invalid(code),
    }
}

fn applied(result: Response) -> RequestResult {
    RequestResult::Applied(Box::new(AppliedResult { result }))
}
fn rejected(code: &str) -> RequestResult {
    RequestResult::Rejected(ErrorResult {
        error: ApiError {
            code: code.into(),
            message: match code {
                "LOCAL_ONLY" => "This operation requires a local connection.",
                "UPDATE_IN_PROGRESS" => "Daemon handover has begun; new work is not admitted.",
                "UPDATE_ALREADY_PENDING" | "UPDATE_LEASE_OWNED" => {
                    "Another local connection owns the pending update."
                }
                "SESSION_NOT_FOUND" => "The requested session does not exist.",
                "PROJECT_NOT_APPROVED" => "Approve a project on this computer first.",
                "RUNTIME_UNAVAILABLE" => "The requested agent runtime is unavailable.",
                "ID_CONFLICT" => "This session ID belongs to a different create request.",
                "STALE_STREAM" | "STALE_CWD_FENCE" | "STALE_SESSION" => {
                    "Session context changed; read the current state before acting."
                }
                "RESPONSE_TOO_LARGE" => "The result exceeds the protocol frame limit.",
                _ => "This daemon skeleton does not execute this operation.",
            }
            .into(),
        },
    })
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_space_ipc::BuildFlavor;

    struct Fixture {
        host: Host,
        root: PathBuf,
        _temp: tempfile::TempDir,
    }
    impl Fixture {
        fn new(marker: Option<Handover>, hash: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(BuildFlavor::Dev.directory_name());
            let state = State::open(&root, BuildFlavor::Dev).unwrap();
            if let Some(marker) = marker {
                marker.write(&root).unwrap();
            }
            let host = Host::new(state, &root, "boot-fixture".into(), hash.into()).unwrap();
            Self {
                host,
                root,
                _temp: temp,
            }
        }
    }
    fn prepare(mode: UpdateMode) -> Request {
        Request::PrepareUpdate(PrepareUpdateParams {
            mode,
            target_hash: "b".repeat(64),
        })
    }
    fn update(dispatch: Dispatch) -> UpdateState {
        let RequestResult::Applied(applied) = dispatch.result else {
            panic!("applied expected");
        };
        match applied.result {
            Response::Update(value) => value.update,
            Response::Health(value) => value.update,
            _ => panic!("update or health expected"),
        }
    }
    fn marker() -> Handover {
        Handover {
            old_hash: "a".repeat(64),
            target_hash: "b".repeat(64),
            phase: HandoverPhase::Draining,
            migration: None,
        }
    }
    fn subscribe(after: Option<EventCursor>) -> Request {
        Request::SubscribeEvents(SubscribeEventsParams { after })
    }

    #[test]
    fn prepare_checkpoints_before_reply_then_rejects_new_work_and_remote_updates() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let host = &fixture.host;
        let remote = host
            .dispatch(Access::Remote, 2, prepare(UpdateMode::Now))
            .unwrap();
        assert!(
            matches!(remote.result, RequestResult::Rejected(ErrorResult { error: ApiError { ref code, .. } }) if code == "LOCAL_ONLY")
        );
        assert_eq!(Handover::read(&fixture.root).unwrap(), None);
        let dispatch = host
            .dispatch(Access::Local, 1, prepare(UpdateMode::Now))
            .unwrap();
        assert!(dispatch.exit_after_reply);
        assert_eq!(update(dispatch), UpdateState::Draining);
        assert_eq!(Handover::read(&fixture.root).unwrap(), Some(marker()));
        let ClientMessage::Request(work) = decode_client_message(include_bytes!(
            "../../../fixtures/v2/create-session-request.json"
        ))
        .unwrap() else {
            panic!("request expected")
        };
        let rejected = host.dispatch(Access::Local, 2, work.request).unwrap();
        assert!(
            matches!(rejected.result, RequestResult::Rejected(ErrorResult { error: ApiError { ref code, .. } }) if code == "UPDATE_IN_PROGRESS")
        );
        host.disconnected(1);
        assert_eq!(
            update(host.dispatch(Access::Local, 2, Request::GetStatus).unwrap()),
            UpdateState::Draining
        );
    }

    #[test]
    fn pending_connection_loss_publishes_cancel_and_resumes_through_high_water() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let host = &fixture.host;
        host.set_active_for_test(1).unwrap();
        assert_eq!(
            update(
                host.dispatch(Access::Local, 1, prepare(UpdateMode::WhenIdle))
                    .unwrap()
            ),
            UpdateState::Pending
        );
        let mut dispatch = host.dispatch(Access::Local, 2, subscribe(None)).unwrap();
        let (mut events, replay) = dispatch.subscription.take().unwrap();
        assert!(replay.is_empty());
        let RequestResult::Applied(data) = dispatch.result else {
            panic!("applied expected")
        };
        let Response::Subscription(response) = data.result else {
            panic!("subscription expected")
        };
        assert_eq!(response.sequence, 1);
        assert_eq!(response.snapshot.unwrap().update, UpdateState::Pending);
        host.disconnected(1);
        let event = events.try_recv().unwrap();
        assert_eq!(event.sequence, 2);
        assert!(matches!(
            event.event,
            Event::UpdateChanged(UpdateResponse {
                update: UpdateState::None
            })
        ));
        assert!(!host.set_active_for_test(0).unwrap());
        assert_eq!(Handover::read(&fixture.root).unwrap(), None);
        let dispatch = host
            .dispatch(
                Access::Local,
                3,
                subscribe(Some(EventCursor {
                    event_stream_id: "boot-fixture".into(),
                    sequence: 1,
                })),
            )
            .unwrap();
        assert_eq!(dispatch.subscription.unwrap().1, vec![event]);
    }

    #[tokio::test]
    async fn synthetic_busy_idle_transition_persists_marker_and_signals_exit() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let host = &fixture.host;
        host.set_active_for_test(1).unwrap();
        let dispatch = host
            .dispatch(Access::Local, 1, prepare(UpdateMode::WhenIdle))
            .unwrap();
        assert!(!dispatch.exit_after_reply);
        assert_eq!(update(dispatch), UpdateState::Pending);
        assert!(host.set_active_for_test(0).unwrap());
        assert_eq!(Handover::read(&fixture.root).unwrap(), Some(marker()));
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            host.shutdown.notified(),
        )
        .await
        .unwrap();
    }

    #[test]
    fn migration_failure_keeps_admission_fenced_and_does_not_retry_on_health_polls() {
        let mut pending = marker();
        pending.phase = HandoverPhase::Starting;
        let fixture = Fixture::new(Some(pending.clone()), &"b".repeat(64));
        {
            let inner = fixture.host.lock().unwrap();
            inner.state.database.execute_batch("DROP TABLE sessions; DROP TABLE migration_history; CREATE TABLE migration_history (sentinel TEXT); PRAGMA user_version=1;").unwrap();
        }
        pending.phase = HandoverPhase::Committed;
        pending.write(&fixture.root).unwrap();
        assert!(
            fixture
                .host
                .dispatch(Access::Local, 1, Request::GetStatus)
                .is_err()
        );
        let files_after_failure = std::fs::read_dir(&fixture.root).unwrap().count();
        for _ in 0..3 {
            assert!(
                fixture
                    .host
                    .dispatch(Access::Local, 1, Request::GetStatus)
                    .is_err()
            );
        }
        assert_eq!(
            std::fs::read_dir(&fixture.root).unwrap().count(),
            files_after_failure
        );
        let inner = fixture.host.lock().unwrap();
        assert!(inner.provisional && inner.migration_failed);
        assert!(!inner.gate.admits_work());
        assert_eq!(
            inner
                .state
                .database
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn session_idempotency_snapshots_and_events_share_one_cursor() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let host = &fixture.host;
        let params = CreateSessionParams {
            session_id: "session-fixture".into(),
            project_id: "project-fixture".into(),
            runtime_id: "pi".into(),
            expected_cwd_fence: "cwd-fixture".into(),
            start: SessionStart::New,
        };
        let first = host.create_for_test(&params).unwrap();
        let RequestResult::Applied(ref applied) = first else {
            panic!("applied")
        };
        let Response::Session(ref created) = applied.result else {
            panic!("session")
        };
        assert_eq!(created.sequence, 1);
        assert_eq!(created.session.session.state, SessionState::Starting);
        assert_eq!(
            host.dispatch(Access::Local, 1, Request::CreateSession(params.clone()))
                .unwrap()
                .result,
            first
        );
        let mut conflict = params.clone();
        conflict.runtime_id = "codex".into();
        let rejected = host
            .dispatch(Access::Local, 1, Request::CreateSession(conflict))
            .unwrap();
        assert!(
            matches!(rejected.result,RequestResult::Rejected(data) if data.error.code=="ID_CONFLICT")
        );
        let stream = created.session.session.stream_id.clone().unwrap();
        let request = Request::OpenSession(OpenSessionParams {
            session_id: params.session_id.clone(),
            expected_stream_id: stream.clone(),
            expected_cwd_fence: params.expected_cwd_fence.clone(),
        });
        assert_eq!(
            host.dispatch(Access::Local, 1, request).unwrap().result,
            first
        );
        let close = Request::CloseSession(CloseSessionParams {
            session_id: params.session_id.clone(),
            expected_stream_id: stream,
            expected_cwd_fence: params.expected_cwd_fence,
        });
        let closed = host
            .dispatch(Access::Local, 1, close.clone())
            .unwrap()
            .result;
        assert!(matches!(&closed, RequestResult::Dispatched(_)));
        assert_eq!(
            host.dispatch(Access::Local, 2, close.clone())
                .unwrap()
                .result,
            closed
        );
        assert!(
            matches!(host.dispatch(Access::Remote,2,close).unwrap().result,RequestResult::Rejected(data) if data.error.code=="LOCAL_ONLY")
        );
        let subscription = host
            .dispatch(
                Access::Local,
                1,
                subscribe(Some(EventCursor {
                    event_stream_id: "boot-fixture".into(),
                    sequence: 0,
                })),
            )
            .unwrap();
        let (_, events) = subscription.subscription.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        let snapshot = host.dispatch(Access::Local, 1, subscribe(None)).unwrap();
        let RequestResult::Applied(data) = snapshot.result else {
            panic!("snapshot")
        };
        let Response::Subscription(data) = data.result else {
            panic!("snapshot")
        };
        assert_eq!(data.sequence, 2);
        assert_eq!(
            data.snapshot.unwrap().sessions[0].state,
            SessionState::Stopping
        );
    }

    #[test]
    fn racing_duplicate_creates_commit_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(BuildFlavor::Dev.directory_name());
        let host = std::sync::Arc::new(
            Host::new(
                State::open(&root, BuildFlavor::Dev).unwrap(),
                &root,
                "boot-fixture".into(),
                "a".repeat(64),
            )
            .unwrap(),
        );
        let params = CreateSessionParams {
            session_id: "same".into(),
            project_id: "project".into(),
            runtime_id: "pi".into(),
            expected_cwd_fence: "cwd".into(),
            start: SessionStart::New,
        };
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let host = host.clone();
                let params = params.clone();
                std::thread::spawn(move || host.create_for_test(&params).unwrap())
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert!(results.iter().all(|result| result == &results[0]));
        let inner = host.lock().unwrap();
        assert_eq!(inner.sequence, 1);
        assert_eq!(sessions::list(&inner.state.database).unwrap().len(), 1);
    }

    #[test]
    fn oversized_session_lists_and_snapshots_return_a_fixed_size_error() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        for index in 0..400 {
            fixture
                .host
                .create_for_test(&CreateSessionParams {
                    session_id: format!("{index:0128}"),
                    project_id: "p".repeat(128),
                    runtime_id: "r".repeat(128),
                    expected_cwd_fence: "c".repeat(128),
                    start: SessionStart::New,
                })
                .unwrap();
        }
        for request in [Request::ListSessions, subscribe(None)] {
            let reply = fixture.host.dispatch(Access::Local, 1, request).unwrap();
            assert!(
                matches!(reply.result,RequestResult::Rejected(data) if data.error.code=="RESPONSE_TOO_LARGE")
            );
            assert!(reply.subscription.is_none());
        }
    }

    #[test]
    fn history_is_bounded_and_missing_history_requires_a_snapshot() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let host = &fixture.host;
        host.set_active_for_test(1).unwrap();
        for _ in 0..35 {
            host.dispatch(Access::Local, 1, prepare(UpdateMode::WhenIdle))
                .unwrap();
            host.dispatch(Access::Local, 1, Request::CancelUpdate)
                .unwrap();
        }
        assert_eq!(host.lock().unwrap().history.len(), HISTORY);
        let dispatch = host
            .dispatch(
                Access::Local,
                2,
                subscribe(Some(EventCursor {
                    event_stream_id: "boot-fixture".into(),
                    sequence: 0,
                })),
            )
            .unwrap();
        assert!(dispatch.subscription.unwrap().1.is_empty());
        let RequestResult::Applied(data) = dispatch.result else {
            panic!("applied expected")
        };
        let Response::Subscription(response) = data.result else {
            panic!("subscription expected")
        };
        assert_eq!(response.mode, SubscriptionMode::SnapshotRequired);
        assert_eq!(response.sequence, 70);
    }

    #[test]
    fn old_login_launch_clears_unapplied_marker_and_new_binary_stays_fenced_until_commit() {
        let old = Fixture::new(Some(marker()), &"a".repeat(64));
        assert_eq!(
            Handover::read(&old.root).unwrap().unwrap().phase,
            HandoverPhase::NotApplied
        );
        assert_eq!(
            update(
                old.host
                    .dispatch(Access::Local, 1, Request::GetStatus)
                    .unwrap()
            ),
            UpdateState::None
        );
        let new = Fixture::new(Some(marker()), &"b".repeat(64));
        let RequestResult::Applied(applied) = new
            .host
            .dispatch(Access::Local, 1, Request::GetStatus)
            .unwrap()
            .result
        else {
            panic!("expected applied")
        };
        let Response::Health(health) = applied.result else {
            panic!("expected health")
        };
        assert!(!health.ready);
        assert_eq!(health.awaiting_update_confirmation, Some(true));
        assert_eq!(
            update(
                new.host
                    .dispatch(Access::Local, 1, Request::GetStatus)
                    .unwrap()
            ),
            UpdateState::Draining
        );
        let mut committed = marker();
        committed.phase = HandoverPhase::Committed;
        committed.write(&new.root).unwrap();
        assert_eq!(
            update(
                new.host
                    .dispatch(Access::Local, 1, Request::GetStatus)
                    .unwrap()
            ),
            UpdateState::None
        );
        assert!(new.host.lock().unwrap().gate.admits_work());
        let RequestResult::Applied(applied) = new
            .host
            .dispatch(Access::Local, 1, Request::GetStatus)
            .unwrap()
            .result
        else {
            panic!("expected applied")
        };
        let Response::Health(health) = applied.result else {
            panic!("expected health")
        };
        assert!(health.ready);
        assert_eq!(health.awaiting_update_confirmation, None);
    }

    #[test]
    fn normal_shutdown_is_local_only_and_freezes_new_work_without_an_update_marker() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        assert!(
            matches!(fixture.host.dispatch(Access::Remote,1,Request::Shutdown).unwrap().result,RequestResult::Rejected(value) if value.error.code == "LOCAL_ONLY")
        );
        assert!(!fixture.host.normal_shutdown_requested().unwrap());
        let reply = fixture
            .host
            .dispatch(Access::Local, 1, Request::Shutdown)
            .unwrap();
        assert!(reply.exit_after_reply);
        assert!(
            matches!(reply.result, RequestResult::Applied(value) if matches!(value.result,Response::ShutdownAccepted))
        );
        assert!(fixture.host.normal_shutdown_requested().unwrap());
        assert!(
            matches!(fixture.host.dispatch(Access::Local,2,prepare(UpdateMode::Now)).unwrap().result,RequestResult::Rejected(value) if value.error.code == "SHUTDOWN_IN_PROGRESS")
        );
        assert!(Handover::read(&fixture.root).unwrap().is_none());
    }

    #[tokio::test]
    async fn untrustworthy_marker_write_failure_freezes_admission_and_fails_closed() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        std::fs::create_dir(fixture.root.join("handover.json")).unwrap();
        assert!(
            fixture
                .host
                .dispatch(Access::Local, 1, prepare(UpdateMode::Now))
                .is_err()
        );
        assert!(!fixture.host.lock().unwrap().gate.admits_work());
        tokio::time::timeout(
            std::time::Duration::from_millis(10),
            fixture.host.shutdown.notified(),
        )
        .await
        .unwrap();
    }
}
