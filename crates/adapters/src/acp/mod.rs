//! Stable ACP v1 through the pinned official SDK. No launch, auth, filesystem or terminal service.
mod config;
pub mod framing;
mod permissions;
mod transport;
pub use agent_client_protocol::v1::schema;
pub use transport::MAX_RECORD_BYTES;

use agent_client_protocol::{Agent, Channel, Client, schema::ProtocolVersion, v1::ConnectionTo};
use schema::*;
use std::{
    collections::BTreeMap,
    fmt, io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};
use tracing::instrument::WithSubscriber as _;

pub const PACKAGE: &str = "@agentclientprotocol/codex-acp";
pub const PACKAGE_VERSION: &str = "2.5.0";
pub const QUEUE: usize = 8;
pub const MAX_PERMISSIONS: usize = 32;
pub const MAX_COMMAND_BYTES: usize = 65_536;
const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Private adapter observations; never serialize these native values onto the public API.
pub enum Event {
    Opened {
        native_id: String,
        modes: Option<SessionModeState>,
        config: Vec<SessionConfigOption>,
    },
    Update(SessionUpdate),
    Permission {
        id: u64,
        tool: ToolCallUpdate,
        options: Vec<PermissionOption>,
    },
    PermissionClosed(u64),
}

struct State {
    native_id: Option<SessionId>,
    early: Vec<SessionNotification>,
    modes: Option<SessionModeState>,
    config: Vec<SessionConfigOption>,
    config_revision: u64,
    next_permission: u64,
    permissions: BTreeMap<u64, permissions::Pending>,
}
struct Shared {
    state: Mutex<State>,
    events: mpsc::Sender<Event>,
    failure: watch::Sender<Option<Error>>,
    running: AtomicBool,
    cancelling: AtomicBool,
}
impl Shared {
    fn fail(&self, code: &'static str) -> Error {
        self.failure.send_if_modified(|value| {
            if value.is_some() {
                false
            } else {
                *value = Some(Error(code));
                true
            }
        });
        Error(code)
    }
    async fn emit(&self, event: Event) -> Result<()> {
        self.events
            .send(event)
            .await
            .map_err(|_| self.fail("ACP_OBSERVER_CLOSED"))
    }
    async fn notification(&self, notification: SessionNotification) -> Result<()> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| self.fail("ACP_STATE_UNAVAILABLE"))?;
            if let Some(id) = &state.native_id {
                if id != &notification.session_id {
                    return Err(self.fail("ACP_SESSION_MISMATCH"));
                }
            } else {
                if state.early.len() == QUEUE {
                    return Err(self.fail("ACP_STARTUP_OVERFLOW"));
                }
                state.early.push(notification);
                return Ok(());
            }
            match &notification.update {
                SessionUpdate::ConfigOptionUpdate(update) => {
                    state.config = update.config_options.clone();
                    state.config_revision = state
                        .config_revision
                        .checked_add(1)
                        .ok_or_else(|| self.fail("ACP_COUNTER_EXHAUSTED"))?;
                }
                SessionUpdate::CurrentModeUpdate(update) => {
                    if let Some(modes) = &mut state.modes {
                        modes.current_mode_id = update.current_mode_id.clone();
                    }
                }
                _ => {}
            }
        }
        self.emit(Event::Update(notification.update)).await
    }
}

pub struct Session {
    connection: ConnectionTo<Agent>,
    native_id: SessionId,
    shared: Arc<Shared>,
    busy: AtomicBool,
    cancelled: watch::Sender<u64>,
}

struct Operation<'a> {
    session: &'a Session,
    complete: bool,
}
impl Operation<'_> {
    fn finish(mut self) -> Result<()> {
        if let Some(error) = *self.session.shared.failure.borrow() {
            return Err(error);
        }
        self.complete = true;
        Ok(())
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.session.shared.fail("ACP_COMMAND_OUTCOME_UNKNOWN");
        }
        self.session.shared.running.store(false, Ordering::Release);
        self.session
            .shared
            .cancelling
            .store(false, Ordering::Release);
        self.session.busy.store(false, Ordering::Release);
    }
}
impl Session {
    fn begin(&self) -> Result<Operation<'_>> {
        if let Some(error) = *self.shared.failure.borrow() {
            return Err(error);
        }
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error("ACP_BUSY"))?;
        Ok(Operation {
            session: self,
            complete: false,
        })
    }
    /// Host owns the bounded follow-up queue and run fence. ACP has no steer operation.
    /// A dropped in-flight future poisons the connection; it never permits another prompt.
    pub async fn prompt(&self, text: String) -> Result<PromptResponse> {
        if text.is_empty() || text.len() > MAX_COMMAND_BYTES {
            return Err(Error("ACP_PROMPT_INVALID"));
        }
        let operation = self.begin()?;
        self.shared.running.store(true, Ordering::Release);
        let mut cancelled = self.cancelled.subscribe();
        let response = self
            .connection
            .send_request(PromptRequest::new(
                self.native_id.clone(),
                vec![ContentBlock::Text(TextContent::new(text))],
            ))
            .block_task();
        tokio::pin!(response);
        let result = tokio::select! {
            result = &mut response => result,
            _ = cancelled.changed() => tokio::time::timeout(DEADLINE, &mut response).await
                .map_err(|_| self.shared.fail("ACP_CANCEL_OUTCOME_UNKNOWN"))?,
        };
        let result = result.map_err(|_| self.shared.fail("ACP_COMMAND_OUTCOME_UNKNOWN"))?;
        if !self
            .shared
            .state
            .lock()
            .map_err(|_| self.shared.fail("ACP_STATE_UNAVAILABLE"))?
            .permissions
            .is_empty()
        {
            return Err(self.shared.fail("ACP_PERMISSION_UNRESOLVED"));
        }
        operation.finish()?;
        Ok(result)
    }
    /// Queues a cancellation, not a successful stop. Only the prompt response settles the turn.
    pub async fn cancel(&self) -> Result<()> {
        if let Some(error) = *self.shared.failure.borrow() {
            return Err(error);
        }
        if !self.shared.running.load(Ordering::Acquire) {
            return Err(Error("ACP_NOT_RUNNING"));
        }
        if self.shared.cancelling.swap(true, Ordering::AcqRel) {
            return Err(Error("ACP_CANCEL_PENDING"));
        }
        self.connection
            .send_notification(CancelNotification::new(self.native_id.clone()))
            .map_err(|_| self.shared.fail("ACP_CANCEL_OUTCOME_UNKNOWN"))?;
        self.cancelled
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        let pending = {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| self.shared.fail("ACP_STATE_UNAVAILABLE"))?;
            std::mem::take(&mut state.permissions)
        };
        for (id, permission) in pending {
            permission
                .responder
                .respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
                .map_err(|_| self.shared.fail("ACP_APPROVAL_OUTCOME_UNKNOWN"))?;
            self.shared.emit(Event::PermissionClosed(id)).await?;
        }
        Ok(())
    }
    pub async fn set_mode(&self, mode: String) -> Result<()> {
        valid_id(&mode)?;
        {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| self.shared.fail("ACP_STATE_UNAVAILABLE"))?;
            if state.modes.as_ref().map_or(0, |modes| {
                modes
                    .available_modes
                    .iter()
                    .filter(|item| item.id.0.as_ref() == mode)
                    .count()
            }) != 1
            {
                return Err(Error("ACP_MODE_UNSUPPORTED"));
            }
        }
        let operation = self.begin()?;
        request(
            &self.connection,
            SetSessionModeRequest::new(self.native_id.clone(), mode),
        )
        .await
        .map_err(|_| self.shared.fail("ACP_COMMAND_OUTCOME_UNKNOWN"))?;
        operation.finish()?;
        Ok(())
    }
    /// Includes model/effort selectors. IDs and values must come from the agent's advertised options.
    pub async fn set_config(&self, id: String, value: String) -> Result<Vec<SessionConfigOption>> {
        valid_id(&id)?;
        valid_id(&value)?;
        let revision = {
            let state = self
                .shared
                .state
                .lock()
                .map_err(|_| self.shared.fail("ACP_STATE_UNAVAILABLE"))?;
            config::validate(&state.config, &id, &value)?;
            state.config_revision
        };
        let operation = self.begin()?;
        let result = request(
            &self.connection,
            SetSessionConfigOptionRequest::new(self.native_id.clone(), id.clone(), value.as_str()),
        )
        .await
        .map_err(|_| self.shared.fail("ACP_COMMAND_OUTCOME_UNKNOWN"))?;
        if !config::select(&result.config_options, &id)
            .is_ok_and(|select| select.current_value.0.as_ref() == value)
        {
            return Err(self.shared.fail("ACP_CONFIG_UNCONFIRMED"));
        }
        {
            let mut state = self
                .shared
                .state
                .lock()
                .map_err(|_| self.shared.fail("ACP_STATE_UNAVAILABLE"))?;
            if state.config_revision == revision {
                state.config = result.config_options.clone();
            }
        }
        operation.finish()?;
        Ok(result.config_options)
    }
    pub async fn approve(
        &self,
        id: u64,
        decision: cafe_space_protocol::ApprovalDecision,
    ) -> Result<()> {
        self.shared.approve(id, decision).await
    }
}

fn valid_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(Error("ACP_IDENTIFIER_INVALID"))
    } else {
        Ok(())
    }
}
async fn request<T: agent_client_protocol::JsonRpcRequest>(
    connection: &ConnectionTo<Agent>,
    request: T,
) -> Result<T::Response> {
    tokio::time::timeout(DEADLINE, connection.send_request(request).block_task())
        .await
        .map_err(|_| Error("ACP_RESPONSE_TIMEOUT"))?
        .map_err(|_| Error("ACP_REQUEST_FAILED"))
}

/// Drive one private owned-session connection. Input/output must be bounded channels (at most QUEUE).
/// The caller validates cwd/owner/run fences, drains observations, and stops the owned process on return.
/// Returning from `run` ends the connection; EOF, malformed input and observer loss are failures.
pub async fn connect(
    input: mpsc::Receiver<io::Result<String>>,
    output: mpsc::Sender<String>,
    events: mpsc::Sender<Event>,
    cwd: PathBuf,
    resume: Option<String>,
    run: Box<dyn FnOnce(Session) -> futures::future::BoxFuture<'static, Result<()>> + Send>,
) -> Result<()> {
    if !cwd.is_absolute() {
        return Err(Error("ACP_CWD_INVALID"));
    }
    if let Some(id) = &resume {
        valid_id(id)?;
    }
    if input.max_capacity() > QUEUE
        || output.max_capacity() > QUEUE
        || events.max_capacity() > QUEUE
    {
        return Err(Error("ACP_QUEUE_INVALID"));
    }
    let (failure, mut failed) = watch::channel(None);
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            native_id: resume.clone().map(SessionId::new),
            early: Vec::new(),
            modes: None,
            config: Vec::new(),
            config_revision: 0,
            next_permission: 0,
            permissions: BTreeMap::new(),
        }),
        events,
        failure,
        running: AtomicBool::new(false),
        cancelling: AtomicBool::new(false),
    });
    let _failure_lifetime = shared.failure.clone();
    let notifications = shared.clone();
    let approvals = shared.clone();
    let (sdk, transport) = Channel::duplex();
    let client = Client
        .v1()
        .on_receive_notification(
            async move |value: SessionNotification, _| {
                notifications
                    .notification(value)
                    .await
                    .map_err(|_| agent_client_protocol::Error::internal_error())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |value: RequestPermissionRequest, responder, _| {
                approvals
                    .permission(value, responder)
                    .await
                    .map_err(|_| agent_client_protocol::Error::internal_error())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(sdk, async move |connection: ConnectionTo<Agent>| {
            let open = async {
                let initialized = request(
                    &connection,
                    InitializeRequest::new(ProtocolVersion::V1)
                        .client_info(Implementation::new("cafe-space", env!("CARGO_PKG_VERSION"))),
                )
                .await?;
                if initialized.protocol_version != ProtocolVersion::V1 {
                    return Err(Error("ACP_VERSION_UNSUPPORTED"));
                }
                let (native_id, modes, config) = if let Some(id) = resume {
                    if !initialized.agent_capabilities.load_session {
                        return Err(Error("ACP_RESUME_UNSUPPORTED"));
                    }
                    let result =
                        request(&connection, LoadSessionRequest::new(id.clone(), cwd)).await?;
                    (SessionId::new(id), result.modes, result.config_options)
                } else {
                    let result = request(&connection, NewSessionRequest::new(cwd)).await?;
                    (result.session_id, result.modes, result.config_options)
                };
                valid_id(&native_id.to_string())?;
                let early = {
                    let mut state = shared
                        .state
                        .lock()
                        .map_err(|_| Error("ACP_STATE_UNAVAILABLE"))?;
                    state.native_id = Some(native_id.clone());
                    state.modes = modes.clone();
                    if state.config_revision == 0 {
                        state.config = config.clone().unwrap_or_default();
                    }
                    std::mem::take(&mut state.early)
                };
                if early.iter().any(|value| value.session_id != native_id) {
                    return Err(Error("ACP_SESSION_MISMATCH"));
                }
                shared
                    .emit(Event::Opened {
                        native_id: native_id.to_string(),
                        modes,
                        config: config.unwrap_or_default(),
                    })
                    .await?;
                for value in early {
                    shared.notification(value).await?;
                }
                let (cancelled, _) = watch::channel(0);
                run(Session {
                    connection,
                    native_id,
                    shared: shared.clone(),
                    busy: AtomicBool::new(false),
                    cancelled,
                })
                .await?;
                if shared.running.load(Ordering::Acquire) {
                    return Err(Error("ACP_COMMAND_OUTCOME_UNKNOWN"));
                }
                Ok(())
            }
            .await;
            if let Err(error) = open {
                shared.fail(error.0);
                return Err(agent_client_protocol::Error::internal_error());
            }
            Ok(())
        })
        // Native IDs and SDK diagnostics stay private even if the host enables tracing.
        .with_subscriber(tracing::subscriber::NoSubscriber::default());
    let result = tokio::select! {
        biased;
        _ = failed.changed() => Err(failed.borrow().unwrap_or(Error("ACP_CONNECTION_CLOSED"))),
        result = transport::bridge(transport, input, output) => result,
        result = client => result.map_err(|_| Error("ACP_PROTOCOL_FAILED")),
    };
    match *failed.borrow() {
        Some(error) => Err(error),
        None => result,
    }
}

#[cfg(test)]
mod fault_tests;
#[cfg(test)]
mod privacy_tests;
#[cfg(test)]
mod tests;
