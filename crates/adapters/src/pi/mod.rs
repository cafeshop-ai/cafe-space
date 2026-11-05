//! Pi 1.5.0 RPC semantics. No process launch, background queue, credentials or model calls here.
pub mod framing;
pub mod install;
mod models;
mod transcript;
pub use transcript::incomplete_message;
pub const MAX_AVAILABLE_MODELS: usize = 256;

use cafe_space_protocol::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt};

pub const MAX_PENDING: usize = 32;
pub const MAX_MESSAGE_BYTES: usize = 65536;
pub const MAX_PARTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    GetState,
    GetAvailableModels,
    Prompt {
        message: String,
    },
    Steer {
        message: String,
    },
    FollowUp {
        message: String,
    },
    ClearQueue,
    Abort,
    SetModel {
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },
    SetThinkingLevel {
        level: ThinkingLevel,
    },
}
impl Command {
    fn kind(&self) -> &'static str {
        match self {
            Self::GetState => "get_state",
            Self::GetAvailableModels => "get_available_models",
            Self::Prompt { .. } => "prompt",
            Self::Steer { .. } => "steer",
            Self::FollowUp { .. } => "follow_up",
            Self::ClearQueue => "clear_queue",
            Self::Abort => "abort",
            Self::SetModel { .. } => "set_model",
            Self::SetThinkingLevel { .. } => "set_thinking_level",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    Accepted(String),
    Rejected(&'static str),
    Unknown(&'static str),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    State(SessionState),
    Outcome {
        id: String,
        outcome: Outcome,
    },
    Configuration {
        id: String,
        model: Option<cafe_space_protocol::ModelInfo>,
        thinking_level: Option<ThinkingLevel>,
    },
    AvailableModels {
        id: String,
        models: Vec<ModelInfo>,
    },
    Message(TranscriptMessage),
    HistoryTruncated,
    Approval(ApprovalRequest),
    ApprovalDispatched {
        id: String,
    },
    ApprovalClosed {
        id: String,
    },
    LocalUiRequired,
    Notice(&'static str),
}
pub struct Outbound {
    pub id: String,
    pub bytes: Vec<u8>,
}

pub fn capabilities() -> SessionCapabilities {
    SessionCapabilities {
        model_control: true,
        thinking_control: true,
        delivery_control: true,
        session_control: true,
        approval_for_session: false,
        tool_approval: ToolApproval::NeverAsks,
    }
}

pub struct Session {
    native_id: String,
    state: SessionState,
    startup_confirmed: bool,
    in_run: bool,
    queue_cleared: bool,
    counter: u64,
    pending: BTreeMap<String, &'static str>,
    dialogs: BTreeMap<String, String>,
    active_message: Option<TranscriptMessage>,
    active_truncated: bool,
    transcript_gap: bool,
    message_counter: u64,
    tools: BTreeMap<String, TranscriptMessage>,
}
impl Session {
    pub fn new(private_native_id: String) -> Self {
        Self {
            native_id: private_native_id,
            state: SessionState::Starting,
            startup_confirmed: false,
            in_run: false,
            queue_cleared: false,
            counter: 0,
            pending: BTreeMap::new(),
            dialogs: BTreeMap::new(),
            active_message: None,
            active_truncated: false,
            transcript_gap: false,
            message_counter: 0,
            tools: BTreeMap::new(),
        }
    }
    pub fn state(&self) -> &SessionState {
        &self.state
    }
    fn next_id(&mut self, prefix: &str) -> Result<String> {
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or(Error("PI_COUNTER_EXHAUSTED"))?;
        Ok(format!("{prefix}-{}", self.counter))
    }
    fn set_state(&mut self, state: SessionState, observations: &mut Vec<Observation>) {
        if self.state != state {
            self.state = state.clone();
            observations.push(Observation::State(state));
        }
    }
    /// Prepare a command before writing, so a fast response cannot arrive before correlation is installed.
    /// Host must check session/cwd/stream fences and cancellation ownership before calling this method.
    pub fn command(&mut self, command: Command) -> Result<Outbound> {
        if matches!(
            self.state,
            SessionState::Failed(_) | SessionState::Stopped | SessionState::Stopping
        ) {
            return Err(Error("PI_CONNECTION_CLOSED"));
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(Error("PI_COMMAND_LIMIT"));
        }
        if !self.startup_confirmed && !matches!(command, Command::GetState) {
            return Err(Error("PI_NOT_READY"));
        }
        if matches!(command, Command::Abort) && !self.queue_cleared {
            return Err(Error("PI_QUEUE_NOT_CLEARED"));
        }
        let resets_queue = matches!(
            command,
            Command::Prompt { .. }
                | Command::Steer { .. }
                | Command::FollowUp { .. }
                | Command::Abort
                | Command::ClearQueue
        );
        let kind = command.kind();
        let id = self.next_id("rpc")?;
        let mut record = serde_json::to_value(command).map_err(|_| Error("PI_COMMAND_INVALID"))?;
        record["id"] = id.clone().into();
        let bytes = framing::encode(&record)?;
        if resets_queue {
            self.queue_cleared = false;
        }
        self.pending.insert(id.clone(), kind);
        Ok(Outbound { id, bytes })
    }
    pub fn prompt(&mut self, message: String, delivery: DeliveryMode) -> Result<Outbound> {
        let command = if !self.in_run {
            Command::Prompt { message }
        } else {
            match delivery {
                DeliveryMode::Steer => Command::Steer { message },
                DeliveryMode::FollowUp => Command::FollowUp { message },
            }
        };
        self.command(command)
    }
    pub fn expire(&mut self, id: &str) -> Option<Observation> {
        self.pending.remove(id).map(|_| Observation::Outcome {
            id: id.into(),
            outcome: Outcome::Unknown("PI_RESPONSE_TIMEOUT"),
        })
    }
    /// EOF does not prove OS process exit; Host must stop its ownership object separately.
    pub fn disconnected(&mut self, code: &'static str) -> Vec<Observation> {
        let mut observations = Vec::new();
        for (id, _) in std::mem::take(&mut self.pending) {
            observations.push(Observation::Outcome {
                id,
                outcome: Outcome::Unknown("PI_COMMAND_OUTCOME_UNKNOWN"),
            });
        }
        for (id, _) in std::mem::take(&mut self.dialogs) {
            observations.push(Observation::ApprovalClosed { id });
        }
        self.active_message = None;
        self.tools.clear();
        self.state = SessionState::Failed(FailureState { code: code.into() });
        observations.push(Observation::State(self.state.clone()));
        observations
    }
    pub fn observe(&mut self, record: impl Into<framing::Record>) -> Result<Vec<Observation>> {
        if matches!(self.state, SessionState::Failed(_)) {
            return Err(Error("PI_CONNECTION_CLOSED"));
        }
        let result = match record.into() {
            framing::Record::Json(value) => self.apply(value),
            framing::Record::Omitted { bytes } => self.omitted_record(bytes),
        };
        match result {
            Ok(observations) => Ok(observations),
            Err(error) => {
                self.state = SessionState::Failed(FailureState {
                    code: error.0.into(),
                });
                Err(error)
            }
        }
    }
    fn apply(&mut self, record: Value) -> Result<Vec<Observation>> {
        let mut observations = Vec::new();
        match string(&record, "type")? {
            "response" => self.response(&record, &mut observations)?,
            "agent_start" => {
                self.in_run = true;
                if self.startup_confirmed && self.state != SessionState::WaitingForLocalUi {
                    self.set_state(SessionState::Running, &mut observations);
                }
            }
            "agent_settled" => {
                record["aborted"]
                    .as_bool()
                    .ok_or(Error("PI_RECORD_INVALID"))?;
                self.in_run = false;
                observations.extend(self.incomplete_transcripts()?);
                self.active_truncated = false;
                self.transcript_gap = false;
                for (id, _) in std::mem::take(&mut self.dialogs) {
                    observations.push(Observation::ApprovalClosed { id });
                }
                self.set_state(
                    if self.startup_confirmed {
                        SessionState::Ready
                    } else {
                        SessionState::Starting
                    },
                    &mut observations,
                );
            }
            "queue_update" => {
                self.queue_cleared &= record["steering"].as_array().is_some_and(Vec::is_empty)
                    && record["followUp"].as_array().is_some_and(Vec::is_empty);
            }
            "extension_ui_request" => self.ui(&record, &mut observations)?,
            "message_start"
            | "message_update"
            | "message_end"
            | "tool_execution_start"
            | "tool_execution_update"
            | "tool_execution_end" => self.transcript(&record, &mut observations)?,
            "extension_error" => observations.push(Observation::Notice("PI_EXTENSION_ERROR")),
            // agent_end and compaction/retry records never establish idle.
            _ => {}
        }
        Ok(observations)
    }
    fn response(&mut self, record: &Value, observations: &mut Vec<Observation>) -> Result<()> {
        let Some(id) = record["id"].as_str() else {
            return Err(Error("PI_RESPONSE_UNCORRELATED"));
        };
        let Some(&expected) = self.pending.get(id) else {
            observations.push(Observation::Notice("PI_LATE_RESPONSE"));
            return Ok(());
        };
        if string(record, "command")? != expected {
            return Err(Error("PI_RESPONSE_MISMATCH"));
        }
        let success = record["success"]
            .as_bool()
            .ok_or(Error("PI_RECORD_INVALID"))?;
        let outcome = if !success {
            Outcome::Rejected("PI_COMMAND_REJECTED")
        } else if matches!(expected, "prompt" | "steer" | "follow_up") {
            let disposition = string(&record["data"], "disposition")?;
            if !matches!(disposition, "started" | "queued" | "handled")
                || (expected != "prompt" && disposition == "started")
            {
                return Err(Error("PI_DISPOSITION_INVALID"));
            }
            Outcome::Accepted(disposition.into())
        } else {
            if expected == "get_state" {
                let data = &record["data"];
                if string(data, "sessionId")? != self.native_id {
                    return Err(Error("PI_SESSION_IDENTITY_MISMATCH"));
                }
                let streaming = data["isStreaming"]
                    .as_bool()
                    .ok_or(Error("PI_RECORD_INVALID"))?;
                let compacting = data["isCompacting"]
                    .as_bool()
                    .ok_or(Error("PI_RECORD_INVALID"))?;
                let queued = data["pendingMessageCount"]
                    .as_u64()
                    .ok_or(Error("PI_RECORD_INVALID"))?;
                if !self.startup_confirmed
                    && (self.in_run || streaming || compacting || queued != 0)
                {
                    return Err(Error("PI_STARTUP_BUSY"));
                }
                if self.in_run || streaming || compacting || queued != 0 {
                    self.in_run = true;
                    if self.state != SessionState::WaitingForLocalUi {
                        self.set_state(SessionState::Running, observations);
                    }
                } else if !self.startup_confirmed && self.state != SessionState::WaitingForLocalUi {
                    self.set_state(SessionState::Ready, observations);
                }
                self.startup_confirmed = true;
                if data.get("model").is_some() || data.get("thinkingLevel").is_some() {
                    let model = if data["model"].is_null() {
                        None
                    } else {
                        let provider = string(&data["model"], "provider")?;
                        let model_id = string(&data["model"], "id")?;
                        if provider.is_empty()
                            || provider.len() > 128
                            || model_id.is_empty()
                            || model_id.len() > 256
                            || provider.chars().any(char::is_control)
                            || model_id.chars().any(char::is_control)
                        {
                            return Err(Error("PI_CONFIGURATION_INVALID"));
                        }
                        Some(cafe_space_protocol::ModelInfo {
                            provider: provider.into(),
                            id: model_id.into(),
                        })
                    };
                    let thinking_level = if data["thinkingLevel"].is_null() {
                        None
                    } else {
                        Some(
                            serde_json::from_value(data["thinkingLevel"].clone())
                                .map_err(|_| Error("PI_CONFIGURATION_INVALID"))?,
                        )
                    };
                    observations.push(Observation::Configuration {
                        id: id.into(),
                        model,
                        thinking_level,
                    });
                }
            }
            if expected == "get_available_models" {
                observations.push(Observation::AvailableModels {
                    id: id.into(),
                    models: models::project(&record["data"]["models"])?,
                });
            }
            Outcome::Applied
        };
        if expected == "clear_queue" && success {
            self.queue_cleared = true;
        }
        self.pending.remove(id);
        observations.push(Observation::Outcome {
            id: id.into(),
            outcome,
        });
        Ok(())
    }
    fn ui(&mut self, record: &Value, observations: &mut Vec<Observation>) -> Result<()> {
        match string(record, "method")? {
            "confirm" => {
                let native = string(record, "id")?;
                if self.dialogs.len() >= MAX_PENDING {
                    self.set_state(SessionState::WaitingForLocalUi, observations);
                    observations.push(Observation::Notice("PI_DIALOG_LIMIT"));
                    observations.push(Observation::LocalUiRequired);
                    return Ok(());
                }
                if native.len() > 256
                    || native.is_empty()
                    || self.dialogs.values().any(|id| id == native)
                {
                    return Err(Error("PI_DIALOG_INVALID"));
                }
                let mut summary = string(record, "title")?.to_owned();
                if let Some(message) = record["message"].as_str() {
                    summary.push('\n');
                    summary.push_str(message);
                }
                if summary.len() > MAX_MESSAGE_BYTES {
                    // Do not offer approval for a request whose safety-relevant details were hidden.
                    self.set_state(SessionState::WaitingForLocalUi, observations);
                    observations.push(Observation::Notice("PI_CONFIRMATION_TOO_LARGE"));
                    observations.push(Observation::LocalUiRequired);
                    return Ok(());
                }
                let id = self.next_id("approval")?;
                self.dialogs.insert(id.clone(), native.into());
                observations.push(Observation::Approval(ApprovalRequest { id, summary }));
            }
            "select" | "input" | "editor" => {
                self.set_state(SessionState::WaitingForLocalUi, observations);
                observations.push(Observation::LocalUiRequired);
            }
            _ => {}
        }
        Ok(())
    }
    /// A Host cannot offer a confirmation whose full details exceed its snapshot/display budget.
    pub fn require_local_ui(&mut self) -> Option<Observation> {
        if !matches!(
            self.state,
            SessionState::Running | SessionState::WaitingForLocalUi
        ) {
            return None;
        }
        self.state = SessionState::WaitingForLocalUi;
        Some(Observation::State(self.state.clone()))
    }

    /// Pure preparation; no claim of dispatch until the writer confirms the complete record was written.
    pub fn approval_response(&self, id: &str, decision: ApprovalDecision) -> Result<Vec<u8>> {
        if decision == ApprovalDecision::AllowForSession {
            return Err(Error("PI_APPROVAL_SCOPE_UNSUPPORTED"));
        }
        let native = self.dialogs.get(id).ok_or(Error("PI_APPROVAL_NOT_FOUND"))?;
        framing::encode(
            &json!({"type":"extension_ui_response", "id":native, "confirmed": decision == ApprovalDecision::AllowOnce}),
        )
    }
    pub fn approval_written(&mut self, id: &str) -> Result<Observation> {
        self.dialogs
            .remove(id)
            .ok_or(Error("PI_APPROVAL_NOT_FOUND"))?;
        Ok(Observation::ApprovalDispatched { id: id.into() })
    }
}

fn string<'a>(record: &'a Value, key: &str) -> Result<&'a str> {
    record[key].as_str().ok_or(Error("PI_RECORD_INVALID"))
}

#[cfg(test)]
mod display_tests;
#[cfg(test)]
mod tests;
