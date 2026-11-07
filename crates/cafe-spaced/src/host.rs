//! Authoritative update gate, durable state and bounded event history for this daemon boot.

pub use crate::owner_settings::OwnerEnvironmentStatus;
use crate::{
    managed::{Process, Streams},
    native_sessions,
    pi_driver::Driver,
    projects, session_activity as activity, sessions,
    state::State,
    transcripts,
    update::{Access, Gate},
};
use cafe_space_adapters::pi::Observation;

pub(crate) mod pi_commands;
mod pi_launch;
pub(crate) mod pi_models;
use cafe_space_ipc::installation::{Handover, HandoverPhase};
use cafe_space_protocol::*;
pub use pi_models::ModelQueryResult;
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::sync::{Notify, broadcast};

const HISTORY: usize = 64;

pub struct Host {
    inner: Mutex<Inner>,
    next_owner: AtomicU64,
    pub shutdown: Notify,
}

struct Inner {
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
    processes: BTreeMap<String, Process>,
    drivers: BTreeMap<String, Driver>,
    native_directories: BTreeMap<String, (std::fs::File, std::fs::File)>,
    pi_pending: BTreeMap<(String, String), pi_commands::Pending>,
    pi_model_queries: BTreeMap<(String, String), pi_models::Pending>,
    pi_owned_runs: std::collections::BTreeSet<String>,
    pi_approvals: BTreeMap<(String, String), pi_commands::Confirmation>,
    // Drop process ownership before the SQLite instance lock during an abrupt Host drop.
    state: State,
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
                processes: BTreeMap::new(),
                drivers: BTreeMap::new(),
                native_directories: BTreeMap::new(),
                pi_pending: BTreeMap::new(),
                pi_model_queries: BTreeMap::new(),
                pi_owned_runs: std::collections::BTreeSet::new(),
                pi_approvals: BTreeMap::new(),
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
            Request::GetOwnerEnvironment
            | Request::ListModels(_)
            | Request::SetModel(_)
            | Request::SetThinking(_)
            | Request::Prompt(_)
            | Request::Cancel(_)
            | Request::RespondToApproval(_)
            | Request::UpdateOwnerEnvironment(_)
            | Request::AddProject(_)
            | Request::UpdateProject(_)
            | Request::RemoveProject(_)
            | Request::GetProjectRoot(_)
                if access != Access::Local =>
            {
                rejected("LOCAL_ONLY")
            }
            Request::GetStatus => applied(Response::Health(HealthResponse {
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                ready: !inner.provisional && !inner.normal_shutdown,
                update: inner.gate.state(),
                binary_hash: Some(inner.binary_hash.clone()),
                event_stream_id: Some(inner.stream_id.clone()),
                awaiting_update_confirmation: (inner.provisional && !inner.migration_failed)
                    .then_some(true),
            })),
            _ if inner.normal_shutdown => rejected("SHUTDOWN_IN_PROGRESS"),
            Request::Shutdown => {
                if access != Access::Local {
                    rejected("LOCAL_ONLY")
                } else {
                    inner.normal_shutdown = true;
                    if let Err(error) = inner
                        .stop_processes()
                        .and_then(|()| inner.state.checkpoint())
                    {
                        self.shutdown.notify_one();
                        return Err(error);
                    }
                    inner.state.log("DAEMON_STOP_REQUESTED")?;
                    exit_after_reply = true;
                    applied(Response::ShutdownAccepted)
                }
            }
            _ if inner.migration_failed => rejected("SCHEMA_MIGRATION_FAILED"),
            _ if inner.provisional => rejected("UPDATE_IN_PROGRESS"),
            Request::GetOwnerEnvironment => {
                owner_wire_result(crate::owner_settings::wire_status(&inner.root))
            }
            Request::ListProjects => applied(Response::Projects(ProjectList {
                items: projects::list(&inner.state.database).map_err(session_io)?,
            })),
            Request::GetProjectRoot(params) => project_result(
                projects::root(&inner.state.database, &params.project_id)
                    .map(Response::ProjectRoot),
            )?,
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
                    snapshot: if !resumed {
                        Some(SnapshotResponse {
                            update: inner.gate.state(),
                            projects: projects::list(&inner.state.database).map_err(session_io)?,
                            runtimes: vec![],
                            sessions,
                        })
                    } else {
                        None
                    },
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
            Request::UpdateOwnerEnvironment(params) => {
                owner_wire_result(crate::owner_settings::update_wire(&inner.root, params))
            }
            Request::AddProject(params) => {
                inner.check_event_capacity()?;
                let result = projects::add(&inner.state.database, &inner.root, params);
                inner.project_result(result)?
            }
            Request::UpdateProject(params) => {
                inner.check_event_capacity()?;
                let result = projects::update(&inner.state.database, &inner.root, params);
                inner.project_result(result)?
            }
            Request::RemoveProject(params) => {
                inner.check_event_capacity()?;
                let id = params.project_id.clone();
                match projects::remove(&inner.state.database, params) {
                    Ok(()) => {
                        inner.publish_event(Event::ProjectRemoved(ProjectRemovedData {
                            project_id: id,
                        }))?;
                        applied(Response::Empty)
                    }
                    Err(error) => project_result(Err(error))?,
                }
            }
            Request::CreateSession(params) => {
                inner.check_event_capacity()?;
                let result = if sessions::get(&inner.state.database, &params.session_id)
                    .map_err(session_io)?
                    .is_some()
                {
                    sessions::create(&inner.state.database, &params, None, None)
                } else {
                    projects::checked(
                        &inner.state.database,
                        &inner.root,
                        &params.project_id,
                        &params.expected_cwd_fence,
                    )
                    .and_then(|(project, _)| {
                        sessions::create(&inner.state.database, &params, Some(&project), None)
                    })
                };
                inner.session_result(result)?
            }
            Request::OpenSession(params) => {
                let result = sessions::open(
                    &inner.state.database,
                    &params.session_id,
                    &params.expected_stream_id,
                    &params.expected_cwd_fence,
                )
                .and_then(|session| {
                    inner.check_project_fence(&session)?;
                    Ok(session)
                });
                inner.session_result(result.map(|session| (session, false)))?
            }
            Request::CloseSession(params) => {
                inner.check_event_capacity()?;
                let result = sessions::open(
                    &inner.state.database,
                    &params.session_id,
                    &params.expected_stream_id,
                    &params.expected_cwd_fence,
                )
                .and_then(|session| {
                    inner.check_project_fence(&session)?;
                    sessions::close(
                        &inner.state.database,
                        &params.session_id,
                        &params.expected_stream_id,
                        &params.expected_cwd_fence,
                    )
                });
                let stopping = result
                    .as_ref()
                    .ok()
                    .filter(|(session, _)| session.state == SessionState::Stopping)
                    .and_then(|(session, _)| session.stream_id.clone());
                let response = inner.session_result(result)?;
                if let Some(operation_id) = stopping {
                    let previous = inner.gate.clone();
                    inner.stop_process(&params.session_id, SessionStopReason::SessionClosed)?;
                    exit_after_reply = self.commit_transition(&mut inner, previous)?;
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

    /// Adapter-only Rust entry point; not reachable through a runtime selected by the current wire API.
    pub fn launch_session(
        &self,
        params: &CreateSessionParams,
        runtime: &RuntimeSummary,
        program: &Path,
        args: &[OsString],
    ) -> io::Result<(SessionSnapshot, Option<Streams>)> {
        self.lock()?
            .launch_session(params, runtime, program, args, None, None)
    }

    /// Native adapter startup/lifecycle attachment only. Wire admission remains disabled until
    /// prompt, transcript and private resume routing are complete.
    pub fn launch_pi_session(
        &self,
        params: &CreateSessionParams,
        runtime: &RuntimeSummary,
        program: &Path,
        args: &[OsString],
        private_native_id: String,
    ) -> io::Result<SessionSnapshot> {
        if !matches!(params.start, SessionStart::New) {
            return Err(invalid("PI_RESUME_REQUIRES_FACTORY"));
        }
        self.lock()?
            .launch_pi_session(params, runtime, program, args, private_native_id, None)
    }

    /// Observe driver failure before OS exit cleanup, under the same admission lock.
    pub fn poll_processes(&self, now: Instant) -> io::Result<()> {
        let mut inner = self.lock()?;
        let previous = inner.gate.clone();
        let ids: Vec<_> = inner.drivers.keys().cloned().collect();
        for id in ids {
            let observed = inner.drivers.get_mut(&id).expect("owned driver").poll(now);
            let failed = observed
                .iter()
                .any(|value| matches!(value, Observation::State(SessionState::Failed(_))));
            inner.observe_pi(&id, observed)?;
            if failed {
                inner.stop_process(&id, SessionStopReason::AgentExited)?;
            }
        }
        let mut exited = Vec::new();
        for (id, process) in &mut inner.processes {
            if process.try_wait()?.is_some() {
                exited.push(id.clone());
            }
        }
        for id in exited {
            if let Some(driver) = inner.drivers.get_mut(&id) {
                let observed = driver.process_exited();
                inner.observe_pi(&id, observed)?;
                inner.stop_process(&id, SessionStopReason::AgentExited)?;
            } else {
                inner.record_process_exit(&id, SessionStopReason::AgentExited)?;
                inner.processes.remove(&id);
                inner.native_directories.remove(&id);
            }
        }
        inner.refresh_activity();
        if self.commit_transition(&mut inner, previous)? {
            self.shutdown.notify_one();
        }
        Ok(())
    }

    pub fn stop_owned_processes(&self) -> io::Result<()> {
        let mut inner = self.lock()?;
        inner.normal_shutdown = true;
        inner.stop_processes()?;
        if inner.provisional || inner.migration_failed {
            return Ok(());
        }
        inner.state.checkpoint()
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
            let had_processes = !inner.processes.is_empty();
            if let Err(error) = inner.stop_processes() {
                self.shutdown.notify_one();
                return Err(error);
            }
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
                    Ok(current) if current.as_ref() != Some(&marker) && !had_processes => {
                        inner.gate = previous
                    }
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
        let fixture_path = inner.root.parent().unwrap().join("synthetic-project");
        std::fs::create_dir_all(&fixture_path)?;
        let directory = cafe_space_ipc::project_root::inspect(&fixture_path)?;
        inner.state.database.execute("INSERT OR IGNORE INTO projects (id,name,root,identity,cwd_fence) VALUES (?1,'Fixture',?2,?3,?4)",rusqlite::params![params.project_id, directory.path.to_str().unwrap(), directory.identity,params.expected_cwd_fence]).map_err(io::Error::other)?;
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
                tool_approval: ToolApproval::Unknown,
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
    fn launch_session(
        &mut self,
        params: &CreateSessionParams,
        runtime: &RuntimeSummary,
        program: &Path,
        args: &[OsString],
        private_native_id: Option<&str>,
        environment: Option<crate::managed::Environment>,
    ) -> io::Result<(SessionSnapshot, Option<Streams>)> {
        if self.normal_shutdown || self.provisional || !self.gate.admits_work() {
            return Err(invalid("UPDATE_IN_PROGRESS"));
        }
        if sessions::get(&self.state.database, &params.session_id)
            .map_err(session_io)?
            .is_some()
        {
            return sessions::create(&self.state.database, params, None, None)
                .map(|(session, _)| (session, None))
                .map_err(session_io);
        }
        if self.processes.len() >= 32 {
            return Err(invalid("PROCESS_LIMIT"));
        }
        self.check_event_capacity()?;
        if let Some(native_id) = private_native_id {
            for id in self.processes.keys() {
                if native_sessions::get(&self.state.database, id)
                    .map_err(session_io)?
                    .as_deref()
                    == Some(native_id)
                {
                    return Err(invalid("NATIVE_SESSION_IN_USE"));
                }
            }
        }
        let (project, directory) = projects::checked(
            &self.state.database,
            &self.root,
            &params.project_id,
            &params.expected_cwd_fence,
        )
        .map_err(session_io)?;
        let transaction = self
            .state
            .database
            .transaction()
            .map_err(io::Error::other)?;
        let (session, _) = sessions::create(&transaction, params, Some(&project), Some(runtime))
            .map_err(session_io)?;
        if let Some(id) = private_native_id {
            native_sessions::record(&transaction, params, id).map_err(session_io)?;
            if matches!(params.start, SessionStart::Resume(_)) {
                // Current-context hydration may omit pre-compaction history; never claim a full archive.
                transcripts::mark_truncated(&transaction, &session.id).map_err(session_io)?;
            }
        }
        transaction.commit().map_err(io::Error::other)?;
        let process = (|| {
            if let Some(id) = private_native_id {
                let guards = native_sessions::directory(&self.root, id)?;
                self.native_directories.insert(session.id.clone(), guards);
            }
            match environment {
                Some(environment) => {
                    Process::spawn_with_environment(program, args, &directory, environment)
                }
                None => Process::spawn(program, args, &directory),
            }
        })();
        match process {
            Ok(mut process) => {
                let streams = process.take_streams()?;
                self.processes.insert(session.id.clone(), process);
                self.refresh_activity();
                self.session_result(Ok((session.clone(), true)))?;
                Ok((session, Some(streams)))
            }
            Err(_) => {
                let mut failed = session.clone();
                failed.state = SessionState::Failed(FailureState {
                    code: "PROCESS_SPAWN_FAILED".into(),
                });
                let changed = sessions::transition(&self.state.database, &session, failed)
                    .map_err(session_io)?;
                self.session_result(Ok(changed))?;
                self.native_directories.remove(&session.id);
                Err(invalid("PROCESS_SPAWN_FAILED"))
            }
        }
    }

    fn observe_pi(&mut self, id: &str, observed: Vec<Observation>) -> io::Result<()> {
        // shortcut: wire runtime admission remains disabled; connect before M1 wire acceptance.
        let mut outcomes = Vec::new();
        for value in observed {
            if matches!(
                &value,
                Observation::Message(_, _)
                    | Observation::HistoryTruncated
                    | Observation::HistoryLoaded { .. }
            ) {
                let session = sessions::get(&self.state.database, id)
                    .map_err(session_io)?
                    .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
                if matches!(
                    session.state,
                    SessionState::Stopped | SessionState::Failed(_)
                ) {
                    continue;
                }
            }
            match value {
                Observation::Approval(approval) => self.pi_confirmation(id, approval)?,
                Observation::ApprovalClosed { id: native } => {
                    self.pi_confirmation_closed(id, &native)
                }
                Observation::ApprovalDispatched { id: native } => {
                    outcomes.push((native, cafe_space_adapters::pi::Outcome::Applied))
                }
                Observation::State(state) => self.observe_pi_state(id, state)?,
                Observation::Outcome {
                    id: native,
                    outcome,
                } => {
                    outcomes.push((native, outcome));
                }
                Observation::AvailableModels { id: native, models } => {
                    self.pi_models_response(id, &native, models);
                }
                Observation::Configuration {
                    id: native,
                    model,
                    thinking_level,
                } => {
                    self.pi_configuration_response(id, &native, model, thinking_level)?;
                }
                Observation::HistoryLoaded {
                    messages,
                    user_preview,
                    ..
                } => {
                    if let Some(preview) = &user_preview {
                        preview
                            .validate()
                            .map_err(|_| invalid("PI_PREVIEW_INVALID"))?;
                        // The source user message may already be evicted from bounded history.
                        if messages.iter().any(|message| {
                            message.id == preview.message_id && message.role != TranscriptRole::User
                        }) || messages
                            .iter()
                            .rfind(|message| message.role == TranscriptRole::User)
                            .is_some_and(|message| message.id != preview.message_id)
                        {
                            return Err(invalid("PI_PREVIEW_INVALID"));
                        }
                    }
                    let session = sessions::get(&self.state.database, id)
                        .map_err(session_io)?
                        .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
                    if session.state != SessionState::Starting {
                        return Err(invalid("PI_HISTORY_UNAVAILABLE"));
                    }
                    let transaction = self
                        .state
                        .database
                        .transaction()
                        .map_err(io::Error::other)?;
                    // get_messages may exclude pre-compaction history; never claim a complete archive.
                    transcripts::initialize_history(&transaction, id, &messages)
                        .map_err(session_io)?;
                    activity::initialize_history(
                        &transaction,
                        id,
                        &messages,
                        user_preview.as_ref(),
                    )
                    .map_err(session_io)?;
                    let next = sessions::get(&transaction, id)
                        .map_err(session_io)?
                        .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
                    let metadata_changed = next != session;
                    // Keep room for the adapter's following readiness observation as well.
                    Self::reserve_events(
                        self.sequence,
                        messages.len() + 1 + usize::from(metadata_changed),
                    )?;
                    transaction.commit().map_err(io::Error::other)?;
                    for message in messages {
                        self.publish_event(Event::MessageChanged(MessageChangedData {
                            session_id: id.into(),
                            message,
                        }))?;
                    }
                    if metadata_changed {
                        self.publish_event(Event::SessionChanged(Box::new(SessionChangedData {
                            session: next,
                        })))?;
                    }
                }
                Observation::HistoryTruncated => {
                    let transaction = self
                        .state
                        .database
                        .transaction()
                        .map_err(io::Error::other)?;
                    transcripts::mark_truncated(&transaction, id).map_err(session_io)?;
                    transaction.commit().map_err(io::Error::other)?;
                }
                Observation::Message(message, user_preview) => {
                    if let Some(preview) = &user_preview {
                        preview
                            .validate()
                            .map_err(|_| invalid("PI_PREVIEW_INVALID"))?;
                        if message.role != TranscriptRole::User || preview.message_id != message.id
                        {
                            return Err(invalid("PI_PREVIEW_INVALID"));
                        }
                    }
                    let previous = sessions::get(&self.state.database, id)
                        .map_err(session_io)?
                        .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
                    let transaction = self
                        .state
                        .database
                        .transaction()
                        .map_err(io::Error::other)?;
                    // Display equality does not imply equal native user provenance.
                    let changed =
                        transcripts::put(&transaction, id, &message).map_err(session_io)?;
                    activity::observe_message(
                        &transaction,
                        id,
                        &message,
                        user_preview.as_ref(),
                        activity::now(),
                    )
                    .map_err(session_io)?;
                    let next = sessions::get(&transaction, id)
                        .map_err(session_io)?
                        .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
                    let metadata_changed = next != previous;
                    Self::reserve_events(
                        self.sequence,
                        usize::from(changed) + usize::from(metadata_changed),
                    )?;
                    transaction.commit().map_err(io::Error::other)?;
                    if changed {
                        self.publish_event(Event::MessageChanged(MessageChangedData {
                            session_id: id.into(),
                            message,
                        }))?;
                    }
                    if metadata_changed {
                        self.publish_event(Event::SessionChanged(Box::new(SessionChangedData {
                            session: next,
                        })))?;
                    }
                }
                _ => {}
            }
        }
        // State may already have advanced through settlement in this poll; never abort a newer run.
        for (native, outcome) in outcomes {
            self.pi_outcome(id, &native, outcome)?;
        }
        Ok(())
    }

    fn record_pi_state(&mut self, id: &str, mut state: SessionState) -> io::Result<()> {
        let session = sessions::get(&self.state.database, id)
            .map_err(session_io)?
            .ok_or_else(|| invalid("SESSION_NOT_FOUND"))?;
        if matches!(
            session.state,
            SessionState::Stopped | SessionState::Failed(_)
        ) {
            return Ok(());
        }
        if let SessionState::Failed(failure) = &mut state
            && failure.code == "PI_CONNECTION_CLOSED"
        {
            failure.code = "PI_CONNECTION_LOST".into();
        }
        self.check_event_capacity()?;
        let mut next = session.clone();
        next.state = state;
        next.stop_reason = None;
        if next.state == SessionState::Running && next.active_run_id.is_none() {
            next.active_run_id = Some(pi_commands::opaque_id()?);
        }
        if session.state == SessionState::Starting
            && next.state == SessionState::Ready
            && self.drivers.contains_key(id)
        {
            let tool_approval = next.capabilities.tool_approval;
            next.capabilities = cafe_space_adapters::pi::capabilities();
            next.capabilities.tool_approval = tool_approval;
        }
        if !matches!(
            next.state,
            SessionState::Running | SessionState::WaitingForLocalUi | SessionState::Stopping
        ) {
            next.active_run_id = None;
        }
        let transaction = self
            .state
            .database
            .transaction()
            .map_err(io::Error::other)?;
        let interrupted = if matches!(next.state, SessionState::Failed(_)) {
            transcripts::interrupt(&transaction, id).map_err(session_io)?
        } else {
            vec![]
        };
        let changed = sessions::transition(&transaction, &session, next).map_err(session_io)?;
        Self::reserve_events(self.sequence, interrupted.len() + usize::from(changed.1))?;
        transaction.commit().map_err(io::Error::other)?;
        self.session_result(Ok(changed))?;
        for message in interrupted {
            self.publish_event(Event::MessageChanged(MessageChangedData {
                session_id: id.into(),
                message,
            }))?;
        }
        Ok(())
    }

    fn record_process_exit(&mut self, id: &str, reason: SessionStopReason) -> io::Result<()> {
        let Some(session) = sessions::get(&self.state.database, id).map_err(session_io)? else {
            return Err(invalid("SESSION_NOT_FOUND"));
        };
        if matches!(
            session.state,
            SessionState::Stopped | SessionState::Failed(_)
        ) {
            return Ok(());
        }
        self.check_event_capacity()?;
        let mut stopped = session.clone();
        stopped.state = SessionState::Stopped;
        stopped.stop_reason = Some(reason);
        stopped.active_run_id = None;
        let transaction = self
            .state
            .database
            .transaction()
            .map_err(io::Error::other)?;
        let interrupted = transcripts::interrupt(&transaction, id).map_err(session_io)?;
        let result = sessions::transition(&transaction, &session, stopped).map_err(session_io)?;
        Self::reserve_events(self.sequence, interrupted.len() + usize::from(result.1))?;
        transaction.commit().map_err(io::Error::other)?;
        self.session_result(Ok(result))?;
        for message in interrupted {
            self.publish_event(Event::MessageChanged(MessageChangedData {
                session_id: id.into(),
                message,
            }))?;
        }
        Ok(())
    }

    fn stop_process(&mut self, id: &str, reason: SessionStopReason) -> io::Result<()> {
        if let Some(process) = self.processes.get_mut(id) {
            process.stop()?;
            self.pi_approvals.retain(|(session, _), _| session != id);
            self.record_process_exit(id, reason)?;
            self.processes.remove(id);
            self.drivers.remove(id);
            self.native_directories.remove(id);
            self.pi_forget(id);
            self.refresh_activity();
        }
        Ok(())
    }

    fn stop_processes(&mut self) -> io::Result<()> {
        let ids: Vec<_> = self.processes.keys().cloned().collect();
        let mut failure = None;
        for id in ids {
            if let Err(error) = self.stop_process(&id, SessionStopReason::AgentExited) {
                // One failed terminal write must not leave other owned trees alive.
                failure.get_or_insert(error);
            }
        }
        self.refresh_activity();
        failure.map_or(Ok(()), Err)
    }

    fn refresh_activity(&mut self) {
        let busy = self
            .processes
            .keys()
            .filter(|id| {
                self.drivers
                    .get(*id)
                    .is_none_or(|driver| driver.state() != &SessionState::Ready)
                    || self.pi_owned_runs.contains(*id)
                    || self
                        .pi_pending
                        .keys()
                        .chain(self.pi_model_queries.keys())
                        .chain(self.pi_approvals.keys())
                        .any(|(session, _)| session == *id)
            })
            .count();
        self.gate.busy_work_changed(busy);
    }

    fn check_project_fence(&self, session: &SessionSnapshot) -> sessions::Result<()> {
        let root = projects::root(&self.state.database, &session.project_id)?;
        if root.cwd_fence != session.cwd_fence {
            return Err(sessions::Error::Rejected("STALE_CWD_FENCE"));
        }
        Ok(())
    }

    fn project_result(
        &mut self,
        result: sessions::Result<(ProjectSummary, bool)>,
    ) -> io::Result<RequestResult> {
        match result {
            Ok((project, changed)) => {
                if changed {
                    self.publish_event(Event::ProjectChanged(ProjectChangedData {
                        project: project.clone(),
                    }))?;
                }
                Ok(applied(Response::Project(project)))
            }
            Err(error) => project_result(Err(error)),
        }
    }

    fn session_result(
        &mut self,
        result: sessions::Result<(SessionSnapshot, bool)>,
    ) -> io::Result<RequestResult> {
        match result {
            Ok((session, changed)) => {
                let (messages, history_truncated) =
                    transcripts::load(&self.state.database, &session.id).map_err(session_io)?;
                if changed {
                    self.publish_event(Event::SessionChanged(Box::new(SessionChangedData {
                        session: session.clone(),
                    })))?;
                }
                Ok(applied(Response::Session(Box::new(SessionResponse {
                    session: SessionView {
                        approvals: self
                            .pi_approvals
                            .iter()
                            .filter(|((id, _), _)| id == &session.id)
                            .map(|(_, confirmation)| confirmation.request.clone())
                            .collect(),
                        session,
                        messages,
                        history_truncated,
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
        Self::reserve_events(self.sequence, 1)
    }

    fn reserve_events(sequence: u32, count: usize) -> io::Result<()> {
        if u32::try_from(count)
            .ok()
            .and_then(|count| sequence.checked_add(count))
            .is_none_or(|last| last > MAX_EVENT_SEQUENCE)
        {
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
            // Retry only the recovery marker if disk I/O failed, never another migration/backup.
            if crate::migrations::require_recovery(&self.root).is_err() {
                self.state.log("SCHEMA_RECOVERY_MARKER_FAILED")?;
            }
            return Ok(());
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
                self.migration_failed = true;
                if crate::migrations::require_recovery(&self.root).is_err() {
                    self.state.log("SCHEMA_RECOVERY_MARKER_FAILED")?;
                }
                self.state.log("SCHEMA_MIGRATION_FAILED")?;
                return Ok(());
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

fn project_result(result: sessions::Result<Response>) -> io::Result<RequestResult> {
    match result {
        Ok(response) => Ok(applied(response)),
        Err(sessions::Error::Rejected(code)) => Ok(rejected(code)),
        Err(error) => Err(session_io(error)),
    }
}

fn applied(result: Response) -> RequestResult {
    RequestResult::Applied(Box::new(AppliedResult { result }))
}
fn owner_wire_result(result: io::Result<OwnerEnvironment>) -> RequestResult {
    match result {
        Ok(status) => applied(Response::OwnerEnvironment(status)),
        Err(error) => {
            let code = match error.to_string().as_str() {
                "OWNER_SETTINGS_WRITE_UNKNOWN" => "OWNER_SETTINGS_WRITE_UNKNOWN",
                "OWNER_SETTINGS_UNTRUSTED" => "OWNER_SETTINGS_UNTRUSTED",
                "OWNER_SETTINGS_READ_FAILED" => "OWNER_SETTINGS_READ_FAILED",
                "OWNER_ENVIRONMENT_NOT_DETECTED" => "OWNER_ENVIRONMENT_NOT_DETECTED",
                "OWNER_PATH_REQUIRED" => "OWNER_PATH_REQUIRED",
                "OWNER_PATH_INVALID" => "OWNER_PATH_INVALID",
                _ => "OWNER_SETTINGS_INVALID",
            };
            if code == "OWNER_SETTINGS_WRITE_UNKNOWN" {
                RequestResult::Unknown(ErrorResult {
                    error: ApiError {
                        code: code.into(),
                        message: "Outcome unknown; read state without retrying.".into(),
                    },
                })
            } else {
                rejected(code)
            }
        }
    }
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
                "PROJECT_IN_USE" => {
                    "Close this project's active sessions before editing or removing it."
                }
                "PROJECT_ROOT_CHANGED" => {
                    "The directory changed; reapprove it locally with project update <id> <cwdFence> --root <directory>."
                }
                "PROJECT_ROOT_UNAVAILABLE" => {
                    "The approved directory is unavailable on this computer."
                }
                "PROJECT_ROOT_UNSUPPORTED" => {
                    "Choose an existing local directory outside volume roots and daemon data."
                }
                "PROJECT_ROOT_NOT_UTF8" => "The directory cannot be represented as UTF-8.",
                "PROJECT_ROOT_ALREADY_APPROVED" => {
                    "That directory belongs to another approved project."
                }
                "PROJECT_ROOT_IDENTITY_UNAVAILABLE" => {
                    "The directory identity could not be verified."
                }
                "INVALID_PROJECT" => {
                    "Choose a label of 1 to 128 UTF-8 bytes without control characters."
                }
                "PROJECT_LIMIT" => "The approved project limit has been reached.",
                "OWNER_SETTINGS_INVALID" => "Owner environment input is invalid.",
                "OWNER_SETTINGS_UNTRUSTED" | "OWNER_SETTINGS_READ_FAILED" => "Owner settings could not be read safely; no automatic repair was attempted.",
                "OWNER_ENVIRONMENT_NOT_DETECTED" => "This variable is not present in the daemon environment.",
                "OWNER_PATH_REQUIRED" | "OWNER_PATH_INVALID" => "Configure a nonempty PATH containing only absolute directories.",
                "RUNTIME_UNAVAILABLE" => "The requested agent runtime is unavailable.",
                "ID_CONFLICT" => "This session ID belongs to a different create request.",
                "STALE_STREAM" | "STALE_CWD_FENCE" | "STALE_SESSION" => {
                    "Session context changed; read the current state before acting."
                }
                "SCHEMA_MIGRATION_FAILED" => "Schema migration failed; local recovery is required.",
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
    #[test]
    fn owner_wire_is_local_private_durable_and_emits_no_events() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let host = &fixture.host;
        let mut events = host.lock().unwrap().events.subscribe();
        let set = || {
            Request::UpdateOwnerEnvironment(UpdateOwnerEnvironmentParams {
                name: "HTTPS_PROXY".into(),
                change: OwnerEnvironmentChange::Set {
                    value: "http://synthetic-user:synthetic-password@proxy.invalid:3128/private"
                        .into(),
                },
            })
        };
        for request in [
            Request::GetOwnerEnvironment,
            set(),
            Request::UpdateOwnerEnvironment(UpdateOwnerEnvironmentParams {
                name: "NODE_OPTIONS".into(),
                change: OwnerEnvironmentChange::Remove,
            }),
        ] {
            assert!(
                matches!(host.dispatch(Access::Remote, 1, request).unwrap().result, RequestResult::Rejected(value) if value.error.code == "LOCAL_ONLY")
            );
        }
        assert!(!fixture.root.join("owner-environment.json").exists());
        let result = host.dispatch(Access::Local, 1, set()).unwrap().result;
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("synthetic-user"));
        assert!(!json.contains("synthetic-password"));
        assert!(!json.contains("/private"));
        assert!(json.contains("http://•••:•••@proxy.invalid:3128"));
        assert!(events.try_recv().is_err());
        assert_eq!(host.lock().unwrap().sequence, 0);
        let snapshot = host
            .dispatch(
                Access::Local,
                1,
                Request::SubscribeEvents(SubscribeEventsParams { after: None }),
            )
            .unwrap()
            .result;
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("owner_environment")
        );
        assert!(
            matches!(host.dispatch(Access::Local, 1, Request::ListRuntimes).unwrap().result, RequestResult::Applied(value) if matches!(value.result, Response::Runtimes(ref list) if list.items.is_empty()))
        );
        let before = std::fs::read(fixture.root.join("owner-environment.json")).unwrap();
        host.set_active_for_test(1).unwrap();
        host.dispatch(Access::Local, 1, prepare(UpdateMode::Now))
            .unwrap();
        assert!(
            matches!(host.dispatch(Access::Local, 1, set()).unwrap().result, RequestResult::Rejected(value) if value.error.code == "UPDATE_IN_PROGRESS")
        );
        assert_eq!(
            std::fs::read(fixture.root.join("owner-environment.json")).unwrap(),
            before
        );
        assert!(matches!(
            host.dispatch(Access::Local, 1, Request::GetOwnerEnvironment)
                .unwrap()
                .result,
            RequestResult::Applied(_)
        ));
        let root = fixture.root.clone();
        drop(fixture.host);
        let reopened = Host::new(
            State::open(&root, BuildFlavor::Dev).unwrap(),
            &root,
            "reopened".into(),
            "a".repeat(64),
        )
        .unwrap();
        let result = reopened
            .dispatch(Access::Local, 1, Request::GetOwnerEnvironment)
            .unwrap()
            .result;
        assert!(
            matches!(result, RequestResult::Applied(value) if matches!(value.result, Response::OwnerEnvironment(ref status) if status.configured.iter().any(|entry| entry.name == "HTTPS_PROXY")))
        );
        assert!(matches!(
            owner_wire_result(Err(io::Error::other("OWNER_SETTINGS_WRITE_UNKNOWN"))),
            RequestResult::Unknown(_)
        ));
        assert!(
            !format!(
                "{:?}",
                owner_wire_result(Err(io::Error::other("synthetic-private-error")))
            )
            .contains("synthetic-private-error")
        );
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
    fn durable_project_crud_events_and_current_fences_reach_session_admission() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let directory = fixture.root.parent().unwrap().join("approved");
        let other = fixture.root.parent().unwrap().join("other");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::create_dir(&other).unwrap();
        let request = || {
            Request::AddProject(AddProjectParams {
                root: directory.to_str().unwrap().into(),
                name: Some("Tea".into()),
            })
        };
        let project = |result: RequestResult| {
            let RequestResult::Applied(value) = result else {
                panic!("applied")
            };
            let Response::Project(project) = value.result else {
                panic!("project")
            };
            project
        };
        let first = project(
            fixture
                .host
                .dispatch(Access::Local, 1, request())
                .unwrap()
                .result,
        );
        assert_eq!(
            project(
                fixture
                    .host
                    .dispatch(Access::Local, 1, request())
                    .unwrap()
                    .result
            ),
            first
        );
        assert_eq!(fixture.host.lock().unwrap().sequence, 1);
        let create = CreateSessionParams {
            session_id: "project-session".into(),
            project_id: first.id.clone(),
            runtime_id: "pi".into(),
            expected_cwd_fence: first.cwd_fence.clone(),
            start: SessionStart::New,
        };
        let result = fixture
            .host
            .dispatch(Access::Local, 1, Request::CreateSession(create.clone()))
            .unwrap()
            .result;
        assert!(
            matches!(result,RequestResult::Rejected(ref value) if value.error.code=="RUNTIME_UNAVAILABLE")
        );
        fixture.host.create_for_test(&create).unwrap();
        let session = sessions::get(
            &fixture.host.lock().unwrap().state.database,
            &create.session_id,
        )
        .unwrap()
        .unwrap();
        let mut stopped = session.clone();
        stopped.state = SessionState::Stopped;
        stopped.stop_reason = Some(SessionStopReason::AgentExited);
        sessions::transition(
            &fixture.host.lock().unwrap().state.database,
            &session,
            stopped,
        )
        .unwrap();
        let next = project(
            fixture
                .host
                .dispatch(
                    Access::Local,
                    1,
                    Request::UpdateProject(UpdateProjectParams {
                        project_id: first.id.clone(),
                        expected_cwd_fence: first.cwd_fence.clone(),
                        root: Some(other.to_str().unwrap().into()),
                        name: None,
                    }),
                )
                .unwrap()
                .result,
        );
        assert_ne!(next.cwd_fence, first.cwd_fence);
        for request in [
            Request::OpenSession(OpenSessionParams {
                session_id: session.id.clone(),
                expected_stream_id: session.stream_id.clone().unwrap(),
                expected_cwd_fence: session.cwd_fence.clone(),
            }),
            Request::CloseSession(CloseSessionParams {
                session_id: session.id.clone(),
                expected_stream_id: session.stream_id.clone().unwrap(),
                expected_cwd_fence: session.cwd_fence.clone(),
            }),
        ] {
            let result = fixture
                .host
                .dispatch(Access::Local, 1, request)
                .unwrap()
                .result;
            assert!(
                matches!(result,RequestResult::Rejected(ref value) if value.error.code=="STALE_CWD_FENCE")
            );
        }
        let local_root = fixture
            .host
            .dispatch(
                Access::Local,
                1,
                Request::GetProjectRoot(GetProjectRootParams {
                    project_id: first.id.clone(),
                }),
            )
            .unwrap()
            .result;
        let expected_root = cafe_space_ipc::project_root::inspect(&other).unwrap().path;
        assert!(
            matches!(local_root,RequestResult::Applied(ref value) if matches!(&value.result,Response::ProjectRoot(root) if Path::new(&root.root)==expected_root))
        );
        let snapshot = fixture
            .host
            .dispatch(Access::Remote, 2, subscribe(None))
            .unwrap()
            .result;
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("root"));
        assert!(!json.contains("approved"));
        for event in &fixture.host.lock().unwrap().history {
            assert!(!serde_json::to_string(event).unwrap().contains("root"));
        }
        fixture
            .host
            .dispatch(
                Access::Local,
                1,
                Request::RemoveProject(RemoveProjectParams {
                    project_id: next.id.clone(),
                    expected_cwd_fence: next.cwd_fence,
                }),
            )
            .unwrap();
        assert!(
            sessions::get(&fixture.host.lock().unwrap().state.database, &session.id)
                .unwrap()
                .is_some()
        );
        let root = fixture.root.clone();
        drop(fixture.host);
        let state = State::open(&root, BuildFlavor::Dev).unwrap();
        assert!(projects::list(&state.database).unwrap().is_empty());
        assert!(
            sessions::get(&state.database, &session.id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn project_administration_and_root_reads_are_local_only() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let mut frames: Vec<ClientMessage> =
            serde_json::from_slice(include_bytes!("../../../fixtures/v2/project-requests.json"))
                .unwrap();
        frames.push(
            decode_client_message(include_bytes!(
                "../../../fixtures/v2/get-project-root-request.json"
            ))
            .unwrap(),
        );
        let ServerMessage::Response(expected) = decode_server_message(include_bytes!(
            "../../../fixtures/v2/project-local-only.json"
        ))
        .unwrap() else {
            panic!("response expected")
        };
        let writes = fixture.host.lock().unwrap().state.database.total_changes();
        for frame in frames {
            let ClientMessage::Request(request) = frame else {
                panic!("request expected")
            };
            let result = fixture
                .host
                .dispatch(Access::Remote, 1, request.request)
                .unwrap();
            assert_eq!(result.result, expected.result);
            assert!(result.subscription.is_none());
        }
        let inner = fixture.host.lock().unwrap();
        assert_eq!(inner.state.database.total_changes(), writes);
        assert_eq!(inner.sequence, 0);
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
    fn live_registry_refresh_discards_a_stale_gate_count_even_without_new_exits() {
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
        host.poll_processes(Instant::now()).unwrap();
        assert_eq!(Handover::read(&fixture.root).unwrap(), Some(marker()));
        assert_eq!(host.lock().unwrap().gate.state(), UpdateState::Draining);
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

    #[tokio::test]
    async fn migration_failure_answers_on_the_same_connection_and_requires_explicit_recovery() {
        use cafe_space_ipc::framing::{read_frame, write_frame};
        let mut pending = marker();
        pending.phase = HandoverPhase::Starting;
        let fixture = Fixture::new(Some(pending.clone()), &"b".repeat(64));
        {
            let inner = fixture.host.lock().unwrap();
            inner.state.database.execute_batch("DROP TABLE sessions; DROP TABLE migration_history; CREATE TABLE migration_history (sentinel TEXT); PRAGMA user_version=1;").unwrap();
        }
        pending.phase = HandoverPhase::Committed;
        pending.write(&fixture.root).unwrap();
        let (mut client, server) = tokio::io::duplex(4096);
        let checking = async {
            write_frame(
                &mut client,
                include_bytes!("../../../fixtures/v2/client-hello.json"),
            )
            .await
            .unwrap();
            assert!(matches!(
                decode_server_message(&read_frame(&mut client).await.unwrap().unwrap()).unwrap(),
                ServerMessage::Welcome(_)
            ));
            for _ in 0..3 {
                for (request, wire) in [
                    (
                        Request::GetStatus,
                        include_bytes!(
                            "../../../fixtures/v2/migration-failed-health-response.json"
                        )
                        .as_slice(),
                    ),
                    (
                        Request::ListSessions,
                        include_bytes!(
                            "../../../fixtures/v2/schema-migration-failed-response.json"
                        )
                        .as_slice(),
                    ),
                ] {
                    let expected = decode_server_message(wire).unwrap();
                    let ServerMessage::Response(ref response) = expected else {
                        panic!("response expected")
                    };
                    let message = ClientMessage::Request(ClientRequest {
                        message_type: RequestMessageType::Request,
                        protocol_version: PROTOCOL_VERSION,
                        request_id: response.request_id.clone(),
                        request,
                    });
                    write_frame(&mut client, &encode_client_message(&message).unwrap())
                        .await
                        .unwrap();
                    assert_eq!(
                        decode_server_message(&read_frame(&mut client).await.unwrap().unwrap())
                            .unwrap(),
                        expected
                    );
                }
            }
            assert!(
                matches!(fixture.host.dispatch(Access::Remote, 2, Request::Shutdown).unwrap().result, RequestResult::Rejected(data) if data.error.code=="LOCAL_ONLY")
            );
            // The native Windows stop path must still be able to release the database for rollback.
            write_frame(
                &mut client,
                include_bytes!("../../../fixtures/v2/shutdown-request.json"),
            )
            .await
            .unwrap();
            assert!(
                matches!(decode_server_message(&read_frame(&mut client).await.unwrap().unwrap()).unwrap(), ServerMessage::Response(data) if matches!(&data.result,RequestResult::Applied(data) if data.result==Response::ShutdownAccepted))
            );
        };
        let (served, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(
                crate::api::serve_connection(server, &fixture.host),
                checking
            )
        })
        .await
        .unwrap();
        served.unwrap();
        let marker = Handover::read(&fixture.root).unwrap().unwrap();
        assert_eq!(marker.phase, HandoverPhase::RecoveryRequired);
        let record = marker.migration.unwrap();
        assert!(!record.completed);
        let backup_path = fixture.root.join(record.backup_file);
        let backup = std::fs::read(&backup_path).unwrap();
        {
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
        drop(fixture.host);
        let database = std::fs::read(fixture.root.join("state.sqlite3")).unwrap();
        let files = std::fs::read_dir(&fixture.root).unwrap().count();
        for _ in 0..3 {
            assert_eq!(
                State::open(&fixture.root, BuildFlavor::Dev)
                    .err()
                    .unwrap()
                    .to_string(),
                "SCHEMA_MIGRATION_FAILED"
            );
            assert_eq!(std::fs::read_dir(&fixture.root).unwrap().count(), files);
            assert_eq!(
                std::fs::read(fixture.root.join("state.sqlite3")).unwrap(),
                database
            );
            assert_eq!(std::fs::read(&backup_path).unwrap(), backup);
        }
    }

    #[test]
    fn pi_failure_clears_the_run_and_cannot_be_overwritten_by_exit_or_late_state() {
        for code in [
            "PI_CONNECTION_CLOSED",
            "PI_RECORD_INVALID",
            "PI_RESPONSE_TIMEOUT",
        ] {
            let fixture = Fixture::new(None, &"a".repeat(64));
            let params = CreateSessionParams {
                session_id: "failed-pi".into(),
                project_id: "project-fixture".into(),
                runtime_id: "pi".into(),
                expected_cwd_fence: "cwd-fixture".into(),
                start: SessionStart::New,
            };
            fixture.host.create_for_test(&params).unwrap();
            let mut inner = fixture.host.lock().unwrap();
            inner
                .record_pi_state(&params.session_id, SessionState::Ready)
                .unwrap();
            let ready = sessions::get(&inner.state.database, &params.session_id)
                .unwrap()
                .unwrap();
            let mut running = ready.clone();
            running.state = SessionState::Running;
            running.active_run_id = Some("owned-run".into());
            sessions::transition(&inner.state.database, &ready, running).unwrap();
            inner
                .observe_pi(
                    &params.session_id,
                    vec![Observation::State(SessionState::Failed(FailureState {
                        code: code.into(),
                    }))],
                )
                .unwrap();
            let failed = sessions::get(&inner.state.database, &params.session_id)
                .unwrap()
                .unwrap();
            assert_eq!(
                failed.state,
                SessionState::Failed(FailureState {
                    code: if code == "PI_CONNECTION_CLOSED" {
                        "PI_CONNECTION_LOST"
                    } else {
                        code
                    }
                    .into()
                })
            );
            assert!(failed.active_run_id.is_none());
            assert!(failed.stop_reason.is_none());
            let sequence = inner.sequence;
            inner
                .record_process_exit(&params.session_id, SessionStopReason::AgentExited)
                .unwrap();
            inner
                .record_pi_state(&params.session_id, SessionState::Ready)
                .unwrap();
            assert_eq!(inner.sequence, sequence);
            assert_eq!(
                sessions::get(&inner.state.database, &params.session_id).unwrap(),
                Some(failed)
            );
        }
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
        assert!(created.session.session.last_activity_at.is_some());
        assert!(matches!(
            &host.lock().unwrap().history.front().unwrap().event,
            Event::SessionChanged(data) if data.session == created.session.session
        ));
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
        let current = sessions::get(&host.lock().unwrap().state.database, &params.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.state, SessionState::Stopping);
        assert!(current.last_activity_at >= created.session.session.last_activity_at);
        assert_eq!(data.snapshot.unwrap().sessions, vec![current.clone()]);
        assert!(
            matches!(&events[0].event, Event::SessionChanged(data) if data.session == created.session.session)
        );
        assert!(matches!(&events[1].event, Event::SessionChanged(data) if data.session == current));
        for access in [Access::Local, Access::Remote] {
            let writes = host.lock().unwrap().state.database.total_changes();
            for request in [
                Request::GetSession(GetSessionParams {
                    session_id: current.id.clone(),
                }),
                Request::ListSessions,
                subscribe(None),
            ] {
                let RequestResult::Applied(reply) =
                    host.dispatch(access, 1, request).unwrap().result
                else {
                    panic!("metadata read")
                };
                match reply.result {
                    Response::Session(view) => assert_eq!(view.session.session, current),
                    Response::Sessions(list) => assert_eq!(list.items, vec![current.clone()]),
                    Response::Subscription(subscription) => assert_eq!(
                        subscription.snapshot.unwrap().sessions,
                        vec![current.clone()]
                    ),
                    _ => panic!("metadata response"),
                }
            }
            let replay = host
                .dispatch(
                    access,
                    1,
                    subscribe(Some(EventCursor {
                        event_stream_id: "boot-fixture".into(),
                        sequence: 0,
                    })),
                )
                .unwrap()
                .subscription
                .unwrap()
                .1;
            assert_eq!(replay, events);
            let inner = host.lock().unwrap();
            assert_eq!(inner.sequence, 2);
            assert_eq!(inner.state.database.total_changes(), writes);
        }
    }

    #[test]
    fn observed_activity_matches_events_and_failed_writes_never_publish() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let mut inner = fixture.host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "observed-activity");
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(
                    TranscriptMessage {
                        id: "user-1".into(),
                        role: TranscriptRole::User,
                        status: TranscriptStatus::Complete,
                        parts: vec![MessagePart::Text(TextPart {
                            text: "Synthetic tea".into(),
                        })],
                    },
                    Some(cafe_space_adapters::pi::UserPreview {
                        message_id: "user-1".into(),
                        insertion_order: 1,
                        text: Some("Synthetic tea".into()),
                    }),
                )],
            )
            .unwrap();
        for observation in [
            Observation::State(SessionState::Ready),
            Observation::Configuration {
                id: "readback".into(),
                model: Some(ModelInfo {
                    provider: "synthetic".into(),
                    id: "tea".into(),
                }),
                thinking_level: Some(ThinkingLevel::Low),
            },
        ] {
            inner
                .state
                .database
                .execute(
                    "UPDATE session_activity SET last_activity_at=100 WHERE session_id=?1",
                    [&session.id],
                )
                .unwrap();
            let sequence = inner.sequence;
            inner
                .observe_pi(&session.id, vec![observation.clone()])
                .unwrap();
            let current = sessions::get(&inner.state.database, &session.id)
                .unwrap()
                .unwrap();
            assert!(current.last_activity_at > Some(100));
            assert_eq!(current.preview.as_deref(), Some("Synthetic tea"));
            assert_eq!(inner.sequence, sequence + 1);
            assert!(
                matches!(&inner.history.back().unwrap().event, Event::SessionChanged(data) if data.session == current)
            );
            let writes = inner.state.database.total_changes();
            inner.observe_pi(&session.id, vec![observation]).unwrap();
            assert_eq!(inner.sequence, sequence + 1);
            assert_eq!(inner.state.database.total_changes(), writes);
        }
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(
                    TranscriptMessage {
                        id: "assistant-1".into(),
                        role: TranscriptRole::Assistant,
                        status: TranscriptStatus::Streaming,
                        parts: vec![MessagePart::Text(TextPart {
                            text: "Synthetic partial reply".into(),
                        })],
                    },
                    None,
                )],
            )
            .unwrap();
        inner
            .state
            .database
            .execute(
                "UPDATE session_activity SET last_activity_at=100 WHERE session_id=?1",
                [&session.id],
            )
            .unwrap();
        let previous = sessions::get(&inner.state.database, &session.id)
            .unwrap()
            .unwrap();
        let messages = transcripts::load(&inner.state.database, &session.id).unwrap();
        let receipt = |database: &rusqlite::Connection| {
            database.query_row(
                "SELECT display_digest,user_message_order,preview_digest FROM session_message_receipts WHERE session_id=?1 AND message_id='user-1'",
                [&session.id],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?, row.get::<_, Vec<u8>>(2)?)),
            ).unwrap()
        };
        let previous_receipt = receipt(&inner.state.database);
        let history = inner.history.clone();
        let sequence = inner.sequence;
        let mut received = inner.events.subscribe();
        inner.state.database.execute_batch("CREATE TRIGGER fail_activity BEFORE INSERT ON session_activity BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
        for observation in [
            Observation::Message(
                TranscriptMessage {
                    id: "user-1".into(),
                    role: TranscriptRole::User,
                    status: TranscriptStatus::Complete,
                    parts: vec![MessagePart::Text(TextPart {
                        text: "Uncommitted tea".into(),
                    })],
                },
                Some(cafe_space_adapters::pi::UserPreview {
                    message_id: "user-1".into(),
                    insertion_order: 1,
                    text: Some("Uncommitted tea".into()),
                }),
            ),
            Observation::Configuration {
                id: "changed-readback".into(),
                model: None,
                thinking_level: None,
            },
            Observation::State(SessionState::Failed(FailureState {
                code: "PI_CONNECTION_LOST".into(),
            })),
        ] {
            let error = inner
                .observe_pi(&session.id, vec![observation])
                .unwrap_err();
            assert_eq!(error.to_string(), "SESSION_STORAGE_FAILED");
            assert_eq!(
                sessions::get(&inner.state.database, &session.id).unwrap(),
                Some(previous.clone())
            );
            assert_eq!(
                transcripts::load(&inner.state.database, &session.id).unwrap(),
                messages
            );
            assert_eq!(receipt(&inner.state.database), previous_receipt);
            assert_eq!(inner.history, history);
            assert_eq!(inner.sequence, sequence);
            assert!(matches!(
                received.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
        }
        drop(inner);
        for access in [Access::Local, Access::Remote] {
            let reply = fixture
                .host
                .dispatch(
                    access,
                    1,
                    Request::GetSession(GetSessionParams {
                        session_id: session.id.clone(),
                    }),
                )
                .unwrap();
            assert!(matches!(reply.result, RequestResult::Applied(data)
                if matches!(data.result, Response::Session(ref view) if view.session.session == previous)));
        }
        let inner = fixture.host.lock().unwrap();
        assert_eq!(inner.history, history);
        assert_eq!(inner.sequence, sequence);
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
    fn activity_and_provenance_survive_cache_eviction_and_daemon_recovery() {
        use cafe_space_adapters::pi::UserPreview;
        let fixture = Fixture::new(None, &"a".repeat(64));
        let message = TranscriptMessage {
            id: "message-1".into(),
            role: TranscriptRole::User,
            status: TranscriptStatus::Complete,
            parts: vec![MessagePart::Text(TextPart {
                text: "Synthetic tea".into(),
            })],
        };
        let preview = UserPreview {
            message_id: message.id.clone(),
            insertion_order: 1,
            text: Some("Synthetic tea".into()),
        };
        let expected = {
            let mut inner = fixture.host.lock().unwrap();
            sessions::tests::approved(&inner.state.database, "activity-host");
            inner
                .observe_pi(
                    "activity-host",
                    vec![Observation::Message(message.clone(), Some(preview.clone()))],
                )
                .unwrap();
            inner.state.database.execute("UPDATE session_activity SET last_activity_at=100 WHERE session_id='activity-host'", []).unwrap();
            let tx = inner.state.database.transaction().unwrap();
            for i in 2..=130 {
                let mut later = message.clone();
                later.id = format!("message-{i}");
                later.role = TranscriptRole::Assistant;
                transcripts::put(&tx, "activity-host", &later).unwrap();
            }
            tx.commit().unwrap();
            assert!(
                !transcripts::load(&inner.state.database, "activity-host")
                    .unwrap()
                    .0
                    .iter()
                    .any(|m| m.id == message.id)
            );
            inner
                .observe_pi(
                    "activity-host",
                    vec![Observation::Message(message, Some(preview.clone()))],
                )
                .unwrap();
            let expected = activity::Metadata {
                last_activity_at: Some(100),
                user_preview: Some(preview),
            };
            assert_eq!(
                activity::load(&inner.state.database, "activity-host").unwrap(),
                expected
            );
            expected
        };
        drop(fixture.host);
        let state = State::open(&fixture.root, BuildFlavor::Dev).unwrap();
        let restarted = Host::new(
            state,
            &fixture.root,
            "boot-restarted".into(),
            "a".repeat(64),
        )
        .unwrap();
        let inner = restarted.lock().unwrap();
        assert_eq!(
            activity::load(&inner.state.database, "activity-host").unwrap(),
            expected
        );
        assert_eq!(
            sessions::get(&inner.state.database, "activity-host")
                .unwrap()
                .unwrap()
                .state,
            SessionState::Stopped
        );
    }

    #[test]
    fn duplicate_display_does_not_write_publish_or_require_sequence_capacity() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/pi-rpc/host-storage.json"))
                .unwrap();
        let message: TranscriptMessage =
            serde_json::from_value(contract["before"].clone()).unwrap();
        let mut inner = fixture.host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "duplicate-display-session");
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(message.clone(), None)],
            )
            .unwrap();
        let sequence = inner.sequence;
        let history = inner.history.len();
        let writes = inner.state.database.total_changes();
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(message.clone(), None)],
            )
            .unwrap();
        assert_eq!(inner.sequence, sequence);
        assert_eq!(inner.history.len(), history);
        assert_eq!(inner.state.database.total_changes(), writes);
        inner.sequence = MAX_EVENT_SEQUENCE;
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(message.clone(), None)],
            )
            .unwrap();
        assert_eq!(inner.sequence, MAX_EVENT_SEQUENCE);
        assert_eq!(inner.state.database.total_changes(), writes);
        inner
            .state
            .database
            .execute(
                "UPDATE session_activity SET last_activity_at=100 WHERE session_id=?1",
                [&session.id],
            )
            .unwrap();
        let metadata = activity::load(&inner.state.database, &session.id).unwrap();
        let receipt: Vec<u8> = inner.state.database.query_row(
            "SELECT display_digest FROM session_message_receipts WHERE session_id=?1 AND message_id=?2",
            [&session.id, &message.id], |row| row.get(0),
        ).unwrap();
        let mut changed = message.clone();
        changed.status = TranscriptStatus::Complete;
        assert!(
            inner
                .observe_pi(
                    &session.id,
                    vec![Observation::Message(changed.clone(), None)]
                )
                .is_err()
        );
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0,
            vec![message]
        );
        assert_eq!(inner.history.len(), history);
        assert_eq!(
            activity::load(&inner.state.database, &session.id).unwrap(),
            metadata
        );
        assert_eq!(inner.state.database.query_row(
            "SELECT display_digest FROM session_message_receipts WHERE session_id=?1 AND message_id=?2",
            [&session.id, &changed.id], |row| row.get::<_, Vec<u8>>(0),
        ).unwrap(), receipt);
        inner.sequence = sequence;
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(changed.clone(), None)],
            )
            .unwrap();
        assert_eq!(inner.sequence, sequence + 2);
        assert_eq!(inner.history.len(), history + 2);
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0,
            vec![changed]
        );
    }

    #[test]
    fn invalid_user_preview_is_rejected_before_storage_or_publication() {
        use cafe_space_adapters::pi::UserPreview;
        let fixture = Fixture::new(None, &"a".repeat(64));
        let mut inner = fixture.host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "preview-session");
        let message = TranscriptMessage {
            id: "user-message".into(),
            role: TranscriptRole::User,
            status: TranscriptStatus::Complete,
            parts: vec![MessagePart::Text(TextPart {
                text: "Synthetic user text".into(),
            })],
        };
        let sequence = inner.sequence;
        let history = inner.history.len();
        for (role, id, insertion_order, text) in [
            (
                TranscriptRole::Assistant,
                "user-message",
                1,
                Some("wrong role".into()),
            ),
            (
                TranscriptRole::User,
                "wrong-message",
                1,
                Some("wrong id".into()),
            ),
            (
                TranscriptRole::User,
                "user-message",
                1,
                Some("bad\nline".into()),
            ),
            (
                TranscriptRole::User,
                "user-message",
                1,
                Some("x".repeat(257)),
            ),
            (TranscriptRole::User, "user-message", 0, None),
            (
                TranscriptRole::User,
                "user-message",
                session_activity::MAX_INSERTION_ORDER + 1,
                None,
            ),
        ] {
            let mut invalid_message = message.clone();
            invalid_message.role = role;
            let preview = UserPreview {
                message_id: id.into(),
                insertion_order,
                text,
            };
            let error = inner
                .observe_pi(
                    &session.id,
                    vec![Observation::Message(invalid_message, Some(preview))],
                )
                .unwrap_err();
            assert_eq!(error.to_string(), "PI_PREVIEW_INVALID");
            assert_eq!(inner.sequence, sequence);
            assert_eq!(inner.history.len(), history);
            assert_eq!(
                transcripts::load(&inner.state.database, &session.id).unwrap(),
                (vec![], false)
            );
            assert_eq!(
                sessions::get(&inner.state.database, &session.id).unwrap(),
                Some(session.clone())
            );
        }
        let mut newer = message.clone();
        newer.id = "newer-user-message".into();
        let error = inner
            .observe_pi(
                &session.id,
                vec![Observation::HistoryLoaded {
                    messages: vec![message.clone(), newer],
                    truncated: true,
                    user_preview: Some(UserPreview {
                        message_id: message.id,
                        insertion_order: 1,
                        text: Some("stale history preview".into()),
                    }),
                }],
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "PI_PREVIEW_INVALID");
        assert_eq!(inner.sequence, sequence);
        assert_eq!(inner.history.len(), history);
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id).unwrap(),
            (vec![], false)
        );
    }

    #[test]
    fn history_accepts_valid_preview_after_its_user_display_was_evicted() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let mut inner = fixture.host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "evicted-preview-session");
        let message = TranscriptMessage {
            id: "retained-assistant".into(),
            role: TranscriptRole::Assistant,
            status: TranscriptStatus::Complete,
            parts: vec![MessagePart::Text(TextPart {
                text: "Synthetic reply".into(),
            })],
        };
        let error = inner
            .observe_pi(
                &session.id,
                vec![Observation::HistoryLoaded {
                    messages: vec![message.clone()],
                    truncated: true,
                    user_preview: Some(cafe_space_adapters::pi::UserPreview {
                        message_id: message.id.clone(),
                        insertion_order: 1,
                        text: Some("not user text".into()),
                    }),
                }],
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "PI_PREVIEW_INVALID");
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id).unwrap(),
            (vec![], false)
        );
        inner
            .observe_pi(
                &session.id,
                vec![Observation::HistoryLoaded {
                    messages: vec![message.clone()],
                    truncated: true,
                    user_preview: Some(cafe_space_adapters::pi::UserPreview {
                        message_id: "evicted-user".into(),
                        insertion_order: 9,
                        text: Some("Original user text".into()),
                    }),
                }],
            )
            .unwrap();
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id).unwrap(),
            (vec![message], true)
        );
    }

    #[test]
    fn cached_messages_replay_match_snapshot_and_survive_restart_without_execution() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/pi-rpc/host-storage.json"))
                .unwrap();
        let message: TranscriptMessage =
            serde_json::from_value(contract["before"].clone()).unwrap();
        let interrupted: TranscriptMessage =
            serde_json::from_value(contract["interrupted"].clone()).unwrap();
        let mut inner = fixture.host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "cached-session");
        inner
            .state
            .database
            .execute(
                "UPDATE session_activity SET last_activity_at=100 WHERE session_id=?1",
                [&session.id],
            )
            .unwrap();
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(message.clone(), None)],
            )
            .unwrap();
        let events: Vec<_> = inner.history.iter().cloned().collect();
        assert!(
            matches!(events[0].event, Event::MessageChanged(ref data) if data.message == message)
        );
        let event = events.last().unwrap().clone();
        let session = sessions::get(&inner.state.database, &session.id)
            .unwrap()
            .unwrap();
        assert!(matches!(event.event, Event::SessionChanged(ref data) if data.session == session));
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0,
            vec![message]
        );
        // A transition and all interruption updates reserve sequence space before commit.
        inner.sequence = MAX_EVENT_SEQUENCE - 1;
        assert!(
            inner
                .record_pi_state(
                    &session.id,
                    SessionState::Failed(FailureState {
                        code: "PI_CONNECTION_LOST".into()
                    })
                )
                .is_err()
        );
        assert_eq!(
            sessions::get(&inner.state.database, &session.id).unwrap(),
            Some(session.clone())
        );
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0[0]
                .status,
            TranscriptStatus::Streaming
        );
        inner.sequence = event.sequence;
        drop(inner);
        let dispatch = fixture
            .host
            .dispatch(
                Access::Local,
                1,
                Request::GetSession(GetSessionParams {
                    session_id: session.id.clone(),
                }),
            )
            .unwrap();
        assert!(
            matches!(dispatch.result, RequestResult::Applied(data) if matches!(data.result, Response::Session(ref view) if view.session.messages.len()==1 && view.sequence==event.sequence))
        );
        let replay = fixture
            .host
            .dispatch(
                Access::Local,
                1,
                subscribe(Some(EventCursor {
                    event_stream_id: "boot-fixture".into(),
                    sequence: 0,
                })),
            )
            .unwrap()
            .subscription
            .unwrap()
            .1;
        assert_eq!(replay, events);
        drop(fixture.host);
        let reopened = Host::new(
            State::open(&fixture.root, BuildFlavor::Dev).unwrap(),
            &fixture.root,
            "restart-cache".into(),
            "a".repeat(64),
        )
        .unwrap();
        let mut inner = reopened.lock().unwrap();
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0,
            vec![interrupted.clone()]
        );
        let recovered = sessions::get(&inner.state.database, &session.id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state, SessionState::Stopped);
        assert_eq!(
            recovered.stop_reason,
            Some(SessionStopReason::DaemonRestarted)
        );
        inner
            .observe_pi(
                &session.id,
                vec![Observation::Message(interrupted.clone(), None)],
            )
            .unwrap();
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0,
            vec![interrupted]
        );
        assert!(inner.processes.is_empty());
        assert!(inner.drivers.is_empty());
    }

    #[test]
    fn native_identity_commits_before_spawn_and_invalid_identity_rolls_back_admission() {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let cwd = fixture.root.parent().unwrap().join("native-project");
        std::fs::create_dir(&cwd).unwrap();
        let RequestResult::Applied(response) = fixture
            .host
            .dispatch(
                Access::Local,
                1,
                Request::AddProject(AddProjectParams {
                    name: None,
                    root: cwd.to_str().unwrap().into(),
                }),
            )
            .unwrap()
            .result
        else {
            panic!("project approval")
        };
        let Response::Project(project) = response.result else {
            panic!("project")
        };
        let mut params = CreateSessionParams {
            session_id: "identity-before-spawn".into(),
            project_id: project.id,
            runtime_id: "pi".into(),
            expected_cwd_fence: project.cwd_fence,
            start: SessionStart::New,
        };
        let runtime = RuntimeSummary {
            id: "pi".into(),
            name: "Fixture".into(),
            version: None,
            available: true,
            tool_approval: ToolApproval::NeverAsks,
        };
        let missing = cwd.join("missing-native-executable");
        assert!(
            fixture
                .host
                .launch_pi_session(
                    &params,
                    &runtime,
                    &missing,
                    &[],
                    "private-fixture-native".into()
                )
                .is_err()
        );
        {
            let inner = fixture.host.lock().unwrap();
            assert_eq!(
                native_sessions::get(&inner.state.database, &params.session_id)
                    .unwrap()
                    .as_deref(),
                Some("private-fixture-native")
            );
            assert!(
                matches!(sessions::get(&inner.state.database, &params.session_id).unwrap().unwrap().state, SessionState::Failed(ref failure) if failure.code == "PROCESS_SPAWN_FAILED")
            );
            assert!(inner.native_directories.is_empty());
            assert!(inner.processes.is_empty());
            assert!(
                !serde_json::to_string(&inner.history)
                    .unwrap()
                    .contains("private-fixture-native")
            );
        }
        params.session_id = "invalid-native-identity".into();
        assert!(
            fixture
                .host
                .launch_pi_session(&params, &runtime, &missing, &[], "../outside".into())
                .is_err()
        );
        assert!(
            sessions::get(
                &fixture.host.lock().unwrap().state.database,
                &params.session_id
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn failed_native_state_interrupts_cached_display_before_publishing_and_late_messages_are_ignored()
     {
        let fixture = Fixture::new(None, &"a".repeat(64));
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/pi-rpc/host-storage.json"))
                .unwrap();
        let before: TranscriptMessage = serde_json::from_value(contract["before"].clone()).unwrap();
        let after: TranscriptMessage =
            serde_json::from_value(contract["interrupted"].clone()).unwrap();
        let mut inner = fixture.host.lock().unwrap();
        let session = sessions::tests::approved(&inner.state.database, "failed-cache");
        inner
            .state
            .database
            .execute(
                "UPDATE session_activity SET last_activity_at=100 WHERE session_id=?1",
                [&session.id],
            )
            .unwrap();
        inner
            .observe_pi(
                &session.id,
                vec![
                    Observation::HistoryTruncated,
                    Observation::Message(before.clone(), None),
                ],
            )
            .unwrap();
        assert!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .1
        );
        inner
            .record_pi_state(
                &session.id,
                SessionState::Failed(FailureState {
                    code: "PI_CONNECTION_CLOSED".into(),
                }),
            )
            .unwrap();
        assert_eq!(
            transcripts::load(&inner.state.database, &session.id)
                .unwrap()
                .0,
            vec![after.clone()]
        );
        assert_eq!(inner.history.len(), 4);
        assert!(
            matches!(inner.history[2].event, Event::SessionChanged(ref data) if matches!(data.session.state, SessionState::Failed(ref failure) if failure.code=="PI_CONNECTION_LOST"))
        );
        assert!(
            matches!(inner.history[3].event, Event::MessageChanged(ref data) if data.message==after)
        );
        inner
            .observe_pi(&session.id, vec![Observation::Message(before, None)])
            .unwrap();
        assert_eq!(inner.history.len(), 4);
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
