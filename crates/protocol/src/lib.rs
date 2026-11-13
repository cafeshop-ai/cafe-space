//! Local API family v2, full wire v9 types shared by the daemon, CLI, and UI.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use ts_rs::TS;

pub mod control;
pub mod owner_environment;
pub mod session_activity;
pub use owner_environment::*;

pub const PROTOCOL_VERSION: u16 = 9;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_EVENT_SEQUENCE: u32 = 1_000_000_000;
const MAX_ID_BYTES: usize = 128;
const MAX_TEXT_BYTES: usize = 65_536;
const MAX_LABEL_BYTES: usize = 256;
const MAX_ITEMS: usize = 1_000;
const MAX_MESSAGES: usize = 1_000;
const MAX_PARTS: usize = 500;
const MAX_APPROVALS: usize = 64;
const MAX_PROTOCOL_VERSIONS: usize = 8;
pub const MAX_REPLAY_EVENTS: u32 = 4_096;
pub const MAX_MODELS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum ClientMessage {
    Hello(ClientHello),
    Request(ClientRequest),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ClientHello {
    #[serde(rename = "type")]
    pub message_type: HelloMessageType,
    pub supported_protocol_versions: Vec<u16>,
    pub client_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HelloMessageType {
    Hello,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WelcomeMessageType {
    Welcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum VersionMismatchMessageType {
    VersionMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ClientRequest {
    #[serde(rename = "type")]
    pub message_type: RequestMessageType,
    #[ts(type = "9")]
    pub protocol_version: u16,
    pub request_id: String,
    pub request: Request,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RequestMessageType {
    Request,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum Request {
    GetStatus,
    GetRecoveryDetails,
    RestoreBackup(RestoreBackupParams),
    Shutdown,
    PrepareUpdate(PrepareUpdateParams),
    CancelUpdate,
    GetOwnerEnvironment,
    UpdateOwnerEnvironment(UpdateOwnerEnvironmentParams),
    ListProjects,
    GetProjectRoot(GetProjectRootParams),
    AddProject(AddProjectParams),
    UpdateProject(UpdateProjectParams),
    RemoveProject(RemoveProjectParams),
    ListRuntimes,
    DiscoverPiRuntime,
    RegisterPiRuntime(RegisterPiRuntimeParams),
    UnregisterPiRuntime,
    ListSessions,
    SubscribeEvents(SubscribeEventsParams),
    ListAttachTargets(ListAttachTargetsParams),
    GetSession(GetSessionParams),
    CreateSession(CreateSessionParams),
    OpenSession(OpenSessionParams),
    ListModels(OpenSessionParams),
    SetModel(SetModelParams),
    SetThinking(SetThinkingParams),
    Prompt(PromptParams),
    Cancel(CancelParams),
    RespondToApproval(RespondToApprovalParams),
    CloseSession(CloseSessionParams),
}

/// Explicit local-owner paths; never included in runtime summaries, events or Debug output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RegisterPiRuntimeParams {
    pub node: String,
    pub package_root: String,
}
impl fmt::Debug for RegisterPiRuntimeParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RegisterPiRuntimeParams([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct PiRuntimeDiscovery {
    pub node: Option<String>,
    pub package_root: Option<String>,
}
impl fmt::Debug for PiRuntimeDiscovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PiRuntimeDiscovery([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct PrepareUpdateParams {
    pub mode: UpdateMode,
    pub target_hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UpdateMode {
    Now,
    WhenIdle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UpdateRecoveryAction {
    Finish,
    Rollback,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UpdateState {
    #[default]
    None,
    Pending,
    Draining,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct UpdateResponse {
    pub update: UpdateState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ListAttachTargetsParams {
    pub project_id: String,
    pub runtime_id: String,
}

/// Root paths are restricted to local administration; shared lists/events never contain them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AddProjectParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct UpdateProjectParams {
    pub project_id: String,
    pub expected_cwd_fence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RemoveProjectParams {
    pub project_id: String,
    pub expected_cwd_fence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct GetProjectRootParams {
    pub project_id: String,
}

/// Only a response to the local-only get_project_root; never emitted to subscribers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ProjectRoot {
    pub project_id: String,
    pub cwd_fence: String,
    pub root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct GetSessionParams {
    pub session_id: String,
}

/// The client-chosen session ID is stable so a lost create response can be reconciled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct CreateSessionParams {
    pub session_id: String,
    pub project_id: String,
    pub runtime_id: String,
    pub expected_cwd_fence: String,
    pub start: SessionStart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct OpenSessionParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SetModelParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
    pub provider: String,
    pub model_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SetThinkingParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
    pub level: ThinkingLevel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct PromptParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
    pub content: String,
    pub delivery: DeliveryMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DeliveryMode {
    Steer,
    FollowUp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct CancelParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
    pub run_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RespondToApprovalParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
    pub approval_id: String,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct CloseSessionParams {
    pub session_id: String,
    pub expected_stream_id: String,
    pub expected_cwd_fence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SubscribeEventsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<EventCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct EventCursor {
    pub event_stream_id: String,
    pub sequence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SubscriptionMode {
    Resumed,
    SnapshotRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "mode",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum SessionStart {
    New,
    Resume(ResumeStartParams),
    Attach(AttachStartParams),
}

/// The Café Space session ID, never the agent's native session ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ResumeStartParams {
    pub source_session_id: String,
}

/// The opaque key issued by `list_attach_targets`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AttachStartParams {
    pub target_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum ServerMessage {
    Welcome(ServerWelcome),
    VersionMismatch(VersionMismatch),
    Response(ServerResponse),
    Event(ServerEvent),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ServerWelcome {
    #[serde(rename = "type")]
    pub message_type: WelcomeMessageType,
    #[ts(type = "9")]
    pub protocol_version: u16,
    pub daemon_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct VersionMismatch {
    #[serde(rename = "type")]
    pub message_type: VersionMismatchMessageType,
    pub daemon_version: String,
    pub supported_protocol_versions: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ServerResponse {
    #[serde(rename = "type")]
    pub message_type: ResponseMessageType,
    #[ts(type = "9")]
    pub protocol_version: u16,
    pub request_id: String,
    pub result: RequestResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ResponseMessageType {
    Response,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ServerEvent {
    #[serde(rename = "type")]
    pub message_type: EventMessageType,
    #[ts(type = "9")]
    pub protocol_version: u16,
    pub event_stream_id: String,
    pub sequence: u32,
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EventMessageType {
    Event,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "status",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum RequestResult {
    Applied(Box<AppliedResult>),
    Rejected(ErrorResult),
    Dispatched(DispatchedResult),
    Unknown(ErrorResult),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AppliedResult {
    pub result: Response,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ErrorResult {
    pub error: ApiError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct DispatchedResult {
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum Response {
    Health(HealthResponse),
    RecoveryDetails(RecoveryDetails),
    ShutdownAccepted,
    Update(UpdateResponse),
    Projects(ProjectList),
    Project(ProjectSummary),
    ProjectRoot(ProjectRoot),
    OwnerEnvironment(OwnerEnvironment),
    Runtimes(RuntimeList),
    PiRuntimeDiscovery(PiRuntimeDiscovery),
    Models(ModelList),
    AttachTargets(AttachTargetList),
    Subscription(SubscriptionResponse),
    Sessions(SessionList),
    Session(Box<SessionResponse>),
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct HealthResponse {
    pub daemon_version: String,
    pub ready: bool,
    #[serde(default)]
    pub update: UpdateState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_stream_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_update_confirmation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum RecoveryCode {
    SchemaMigrationFailed,
    RollbackRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RecoveryState {
    pub code: RecoveryCode,
    pub from_schema: u32,
    pub to_schema: u32,
    pub backup: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RestoreBackupParams {
    pub confirm: bool,
    pub code: RecoveryCode,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RecoveryDetails {
    pub data_directory: String,
    pub backup_file: Option<String>,
    pub state_files: Vec<String>,
}

impl fmt::Debug for RecoveryDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryDetails([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ProjectList {
    pub items: Vec<ProjectSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RuntimeList {
    pub items: Vec<RuntimeSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AttachTargetList {
    pub items: Vec<AttachTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SubscriptionResponse {
    pub event_stream_id: String,
    pub sequence: u32,
    pub mode: SubscriptionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotResponse>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionView {
    pub session: SessionSnapshot,
    pub messages: Vec<TranscriptMessage>,
    pub history_truncated: bool,
    pub approvals: Vec<ApprovalRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SnapshotResponse {
    #[serde(default)]
    pub update: UpdateState,
    pub projects: Vec<ProjectSummary>,
    pub runtimes: Vec<RuntimeSummary>,
    pub sessions: Vec<SessionSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionList {
    pub items: Vec<SessionSnapshot>,
    pub event_stream_id: String,
    pub sequence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionResponse {
    pub session: SessionView,
    pub event_stream_id: String,
    pub sequence: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub cwd_fence: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RuntimeSummary {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub available: bool,
    pub tool_approval: ToolApproval,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct AttachTarget {
    pub key: String,
    pub project_id: String,
    pub runtime_id: String,
    pub cwd_fence: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionSnapshot {
    pub id: String,
    pub project_id: String,
    pub runtime_id: String,
    pub cwd_fence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    pub capabilities: SessionCapabilities,
    pub mode: SessionMode,
    pub state: SessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<SessionStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(as = "Option<f64>")]
    pub last_activity_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ModelList {
    pub items: Vec<ModelInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionCapabilities {
    pub model_control: bool,
    pub thinking_control: bool,
    pub delivery_control: bool,
    pub session_control: bool,
    pub approval_for_session: bool,
    pub tool_approval: ToolApproval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToolApproval {
    NeverAsks,
    MayAsk,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionMode {
    New,
    Resume,
    Attach,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "state",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum SessionState {
    Starting,
    Ready,
    Running,
    WaitingForLocalUi,
    Stopping,
    Stopped,
    Failed(FailureState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum SessionStopReason {
    DaemonRestarted,
    SessionClosed,
    AgentExited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct FailureState {
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TranscriptRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TranscriptStatus {
    Streaming,
    Complete,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ToolStatus {
    Running,
    Complete,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum MessagePart {
    Text(TextPart),
    Thinking(TextPart),
    ToolCall(ToolCallPart),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct TextPart {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ToolCallPart {
    pub id: String,
    pub name: String,
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub status: ToolStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct TranscriptMessage {
    pub id: String,
    pub role: TranscriptRole,
    pub parts: Vec<MessagePart>,
    pub status: TranscriptStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ApprovalRequest {
    pub id: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowForSession,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[ts(export)]
pub enum Event {
    ProjectChanged(ProjectChangedData),
    ProjectRemoved(ProjectRemovedData),
    SessionChanged(Box<SessionChangedData>),
    SessionRemoved(SessionRemovedData),
    MessageChanged(MessageChangedData),
    ApprovalRequired(ApprovalRequiredData),
    ApprovalResolved(ApprovalResolvedData),
    Notice(NoticeData),
    UpdateChanged(UpdateResponse),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionChangedData {
    pub session: SessionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ProjectChangedData {
    pub project: ProjectSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ProjectRemovedData {
    pub project_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SessionRemovedData {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct MessageChangedData {
    pub session_id: String,
    pub message: TranscriptMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ApprovalRequiredData {
    pub session_id: String,
    pub approval: ApprovalRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct ApprovalResolvedData {
    pub session_id: String,
    pub approval_id: String,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct NoticeData {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    FrameTooLarge,
    InvalidFrame,
    InvalidMessage,
    UnsupportedVersion(u16),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge => f.write_str("protocol frame exceeds the size limit"),
            Self::InvalidFrame => f.write_str("invalid protocol frame"),
            Self::InvalidMessage => f.write_str("invalid protocol message"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported protocol version {version}")
            }
        }
    }
}

impl Error for ProtocolError {}

pub fn decode_client_message(frame: &[u8]) -> Result<ClientMessage, ProtocolError> {
    let message = decode_json(frame)?;
    validate_client_message(&message)?;
    Ok(message)
}

pub fn decode_server_message(frame: &[u8]) -> Result<ServerMessage, ProtocolError> {
    let message = decode_json(frame)?;
    validate_server_message(&message)?;
    Ok(message)
}

pub fn encode_client_message(message: &ClientMessage) -> Result<Vec<u8>, ProtocolError> {
    validate_client_message(message)?;
    encode_json(message)
}

pub fn encode_server_message(message: &ServerMessage) -> Result<Vec<u8>, ProtocolError> {
    validate_server_message(message)?;
    encode_json(message)
}

fn decode_json<T: DeserializeOwned>(frame: &[u8]) -> Result<T, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    serde_json::from_slice(frame).map_err(|_| ProtocolError::InvalidFrame)
}

fn encode_json<T: Serialize>(message: &T) -> Result<Vec<u8>, ProtocolError> {
    let frame = serde_json::to_vec(message).map_err(|_| ProtocolError::InvalidFrame)?;
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(frame)
}

fn validate_client_message(message: &ClientMessage) -> Result<(), ProtocolError> {
    let ClientRequest {
        protocol_version,
        request_id,
        request,
        ..
    } = match message {
        ClientMessage::Hello(hello) => {
            validate_protocol_versions(&hello.supported_protocol_versions)?;
            check_text(&hello.client_version, MAX_LABEL_BYTES, false)?;
            return Ok(());
        }
        ClientMessage::Request(request) => request,
    };
    validate_version(*protocol_version)?;
    check_id(request_id)?;

    match request {
        Request::GetStatus
        | Request::GetRecoveryDetails
        | Request::RestoreBackup(_)
        | Request::Shutdown
        | Request::ListProjects
        | Request::GetOwnerEnvironment
        | Request::ListRuntimes
        | Request::DiscoverPiRuntime
        | Request::UnregisterPiRuntime
        | Request::ListSessions
        | Request::CancelUpdate => {}
        Request::UpdateOwnerEnvironment(params) => owner_environment::validate_update(params)?,
        Request::RegisterPiRuntime(params) => {
            check_id_max(&params.node, MAX_TEXT_BYTES)?;
            check_id_max(&params.package_root, MAX_TEXT_BYTES)?;
        }
        Request::GetProjectRoot(params) => check_id(&params.project_id)?,
        Request::AddProject(params) => {
            if let Some(name) = &params.name {
                check_id(name)?;
            }
            check_id_max(&params.root, MAX_TEXT_BYTES)?;
        }
        Request::UpdateProject(params) => {
            check_ids(&[&params.project_id, &params.expected_cwd_fence])?;
            if let Some(name) = &params.name {
                check_id(name)?;
            }
            if let Some(root) = &params.root {
                check_id_max(root, MAX_TEXT_BYTES)?;
            }
        }
        Request::RemoveProject(params) => {
            check_ids(&[&params.project_id, &params.expected_cwd_fence])?;
        }
        Request::PrepareUpdate(params) => check_binary_hash(&params.target_hash)?,
        Request::SubscribeEvents(params) => {
            if let Some(after) = &params.after {
                check_id(&after.event_stream_id)?;
                check_sequence(after.sequence)?;
            }
        }
        Request::ListAttachTargets(params) => check_ids(&[&params.project_id, &params.runtime_id])?,
        Request::GetSession(params) => check_id(&params.session_id)?,
        Request::CreateSession(params) => {
            check_ids(&[
                &params.session_id,
                &params.project_id,
                &params.runtime_id,
                &params.expected_cwd_fence,
            ])?;
            match &params.start {
                SessionStart::New => {}
                SessionStart::Resume(start) => check_id(&start.source_session_id)?,
                SessionStart::Attach(start) => check_id(&start.target_key)?,
            }
        }
        Request::OpenSession(params) | Request::ListModels(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
            ])?;
        }
        Request::SetModel(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
            ])?;
            check_id(&params.provider)?;
            check_id_max(&params.model_id, MAX_LABEL_BYTES)?;
        }
        Request::SetThinking(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
            ])?;
        }
        Request::CloseSession(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
            ])?;
        }
        Request::Prompt(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
            ])?;
            check_text(&params.content, MAX_TEXT_BYTES, false)?;
        }
        Request::Cancel(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
                &params.run_id,
            ])?;
        }
        Request::RespondToApproval(params) => {
            check_ids(&[
                &params.session_id,
                &params.expected_stream_id,
                &params.expected_cwd_fence,
                &params.approval_id,
            ])?;
        }
    }
    Ok(())
}

fn validate_server_message(message: &ServerMessage) -> Result<(), ProtocolError> {
    match message {
        ServerMessage::Welcome(welcome) => {
            validate_version(welcome.protocol_version)?;
            check_text(&welcome.daemon_version, MAX_LABEL_BYTES, false)?;
        }
        ServerMessage::VersionMismatch(mismatch) => {
            validate_protocol_versions(&mismatch.supported_protocol_versions)?;
            check_text(&mismatch.daemon_version, MAX_LABEL_BYTES, false)?;
        }
        ServerMessage::Response(response) => {
            validate_version(response.protocol_version)?;
            check_id(&response.request_id)?;
            match &response.result {
                RequestResult::Applied(result) => validate_response(&result.result)?,
                RequestResult::Rejected(result) | RequestResult::Unknown(result) => {
                    validate_error(&result.error)?;
                }
                RequestResult::Dispatched(result) => check_id(&result.operation_id)?,
            }
        }
        ServerMessage::Event(event) => {
            validate_version(event.protocol_version)?;
            check_id(&event.event_stream_id)?;
            check_event_sequence(event.sequence)?;
            validate_event(&event.event)?;
        }
    }
    Ok(())
}

fn validate_protocol_versions(versions: &[u16]) -> Result<(), ProtocolError> {
    if versions.is_empty()
        || versions.len() > MAX_PROTOCOL_VERSIONS
        || versions.contains(&0)
        || versions
            .iter()
            .enumerate()
            .any(|(index, version)| versions[..index].contains(version))
    {
        Err(ProtocolError::InvalidMessage)
    } else {
        Ok(())
    }
}

fn validate_version(version: u16) -> Result<(), ProtocolError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedVersion(version))
    }
}

pub fn check_binary_hash(value: &str) -> Result<(), ProtocolError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(ProtocolError::InvalidMessage)
    }
}

fn check_id(value: &str) -> Result<(), ProtocolError> {
    check_id_max(value, MAX_ID_BYTES)
}

fn check_id_max(value: &str, max_bytes: usize) -> Result<(), ProtocolError> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        Err(ProtocolError::InvalidMessage)
    } else {
        Ok(())
    }
}

fn check_event_sequence(sequence: u32) -> Result<(), ProtocolError> {
    if sequence == 0 {
        Err(ProtocolError::InvalidMessage)
    } else {
        check_sequence(sequence)
    }
}

fn check_sequence(sequence: u32) -> Result<(), ProtocolError> {
    if sequence > MAX_EVENT_SEQUENCE {
        Err(ProtocolError::InvalidMessage)
    } else {
        Ok(())
    }
}

fn check_ids(values: &[&str]) -> Result<(), ProtocolError> {
    for value in values {
        check_id(value)?;
    }
    Ok(())
}

fn check_text(value: &str, max_bytes: usize, allow_empty: bool) -> Result<(), ProtocolError> {
    if value.len() > max_bytes || (!allow_empty && value.is_empty()) {
        Err(ProtocolError::InvalidMessage)
    } else {
        Ok(())
    }
}

fn validate_response(response: &Response) -> Result<(), ProtocolError> {
    match response {
        Response::Health(health) => {
            check_text(&health.daemon_version, MAX_LABEL_BYTES, false)?;
            if let Some(hash) = &health.binary_hash {
                check_binary_hash(hash)?;
            }
            if let Some(id) = &health.event_stream_id {
                check_id(id)?;
            }
            if let Some(recovery) = &health.recovery
                && (health.ready
                    || health.awaiting_update_confirmation == Some(true)
                    || recovery.from_schema == 0
                    || recovery.from_schema > recovery.to_schema)
            {
                return Err(ProtocolError::InvalidMessage);
            }
        }
        Response::RecoveryDetails(details) => {
            check_id_max(&details.data_directory, MAX_TEXT_BYTES)?;
            if let Some(backup) = &details.backup_file {
                check_id_max(backup, MAX_TEXT_BYTES)?;
            }
            check_count(details.state_files.len())?;
            for path in &details.state_files {
                check_id_max(path, MAX_TEXT_BYTES)?;
            }
        }
        Response::Update(_) | Response::ShutdownAccepted => {}
        Response::OwnerEnvironment(status) => owner_environment::validate_status(status)?,
        Response::PiRuntimeDiscovery(discovery) => {
            for path in [&discovery.node, &discovery.package_root]
                .into_iter()
                .flatten()
            {
                check_id_max(path, MAX_TEXT_BYTES)?;
            }
        }
        Response::Project(project) => validate_project(project)?,
        Response::ProjectRoot(project) => {
            check_ids(&[&project.project_id, &project.cwd_fence])?;
            check_id_max(&project.root, MAX_TEXT_BYTES)?;
        }
        Response::Projects(list) => {
            check_count(list.items.len())?;
            for project in &list.items {
                validate_project(project)?;
            }
        }
        Response::Runtimes(list) => {
            check_count(list.items.len())?;
            for runtime in &list.items {
                validate_runtime(runtime)?;
            }
        }
        Response::Models(list) => {
            if list.items.len() > MAX_MODELS {
                return Err(ProtocolError::InvalidMessage);
            }
            let mut seen = std::collections::BTreeSet::new();
            for model in &list.items {
                check_id(&model.provider)?;
                check_id_max(&model.id, MAX_LABEL_BYTES)?;
                if !seen.insert((&model.provider, &model.id)) {
                    return Err(ProtocolError::InvalidMessage);
                }
            }
        }
        Response::AttachTargets(list) => {
            check_count(list.items.len())?;
            for target in &list.items {
                check_ids(&[
                    &target.key,
                    &target.project_id,
                    &target.runtime_id,
                    &target.cwd_fence,
                ])?;
                check_text(&target.label, MAX_LABEL_BYTES, false)?;
            }
        }
        Response::Subscription(subscription) => {
            check_id(&subscription.event_stream_id)?;
            check_sequence(subscription.sequence)?;
            match (&subscription.mode, &subscription.snapshot) {
                (SubscriptionMode::Resumed, None) => {}
                (SubscriptionMode::SnapshotRequired, Some(snapshot)) => {
                    validate_snapshot(snapshot)?;
                }
                _ => return Err(ProtocolError::InvalidMessage),
            }
        }
        Response::Sessions(list) => {
            check_id(&list.event_stream_id)?;
            check_sequence(list.sequence)?;
            check_count(list.items.len())?;
            for session in &list.items {
                validate_session(session)?;
            }
        }
        Response::Session(result) => {
            check_id(&result.event_stream_id)?;
            check_sequence(result.sequence)?;
            validate_session_view(&result.session)?;
        }
        Response::Empty => {}
    }
    Ok(())
}

fn validate_snapshot(snapshot: &SnapshotResponse) -> Result<(), ProtocolError> {
    check_count(snapshot.projects.len())?;
    for project in &snapshot.projects {
        validate_project(project)?;
    }
    check_count(snapshot.runtimes.len())?;
    for runtime in &snapshot.runtimes {
        validate_runtime(runtime)?;
    }
    check_count(snapshot.sessions.len())?;
    for session in &snapshot.sessions {
        validate_session(session)?;
    }
    Ok(())
}

fn validate_project(project: &ProjectSummary) -> Result<(), ProtocolError> {
    check_ids(&[&project.id, &project.cwd_fence])?;
    check_id(&project.name)
}

fn validate_runtime(runtime: &RuntimeSummary) -> Result<(), ProtocolError> {
    check_id(&runtime.id)?;
    check_text(&runtime.name, MAX_LABEL_BYTES, false)?;
    check_optional_text(&runtime.version, MAX_LABEL_BYTES)
}

fn validate_session_view(view: &SessionView) -> Result<(), ProtocolError> {
    validate_session(&view.session)?;
    check_limit(view.messages.len(), MAX_MESSAGES)?;
    for message in &view.messages {
        validate_message(message)?;
    }
    check_limit(view.approvals.len(), MAX_APPROVALS)?;
    for approval in &view.approvals {
        validate_approval(approval)?;
    }
    Ok(())
}

fn validate_event(event: &Event) -> Result<(), ProtocolError> {
    match event {
        Event::UpdateChanged(_) => {}
        Event::ProjectChanged(data) => validate_project(&data.project)?,
        Event::ProjectRemoved(data) => check_id(&data.project_id)?,
        Event::SessionChanged(data) => validate_session(&data.session)?,
        Event::SessionRemoved(data) => check_id(&data.session_id)?,
        Event::MessageChanged(data) => {
            check_id(&data.session_id)?;
            validate_message(&data.message)?;
        }
        Event::ApprovalRequired(data) => {
            check_id(&data.session_id)?;
            validate_approval(&data.approval)?;
        }
        Event::ApprovalResolved(data) => {
            check_ids(&[&data.session_id, &data.approval_id])?;
        }
        Event::Notice(data) => {
            check_id(&data.code)?;
            check_text(&data.message, 16 * 1024, false)?;
        }
    }
    Ok(())
}

fn validate_session(session: &SessionSnapshot) -> Result<(), ProtocolError> {
    if session.stop_reason.is_some() && session.state != SessionState::Stopped {
        return Err(ProtocolError::InvalidMessage);
    }
    check_ids(&[
        &session.id,
        &session.project_id,
        &session.runtime_id,
        &session.cwd_fence,
    ])?;
    if let Some(stream_id) = &session.stream_id {
        check_id(stream_id)?;
    }
    check_optional_text(&session.title, MAX_LABEL_BYTES)?;
    if let Some(value) = session.last_activity_at {
        session_activity::validate_activity(value)?;
    }
    if let Some(preview) = &session.preview {
        session_activity::validate_preview(preview)?;
    }
    if let Some(model) = &session.model {
        check_id(&model.provider)?;
        check_id_max(&model.id, MAX_LABEL_BYTES)?;
    }
    if let Some(run_id) = &session.active_run_id {
        check_id(run_id)?;
    }
    if let SessionState::Failed(data) = &session.state {
        check_id(&data.code)?;
    }
    Ok(())
}

fn validate_message(message: &TranscriptMessage) -> Result<(), ProtocolError> {
    check_id(&message.id)?;
    check_limit(message.parts.len(), MAX_PARTS)?;
    for part in &message.parts {
        match part {
            MessagePart::Text(data) | MessagePart::Thinking(data) => {
                check_text(&data.text, MAX_TEXT_BYTES, true)?;
            }
            MessagePart::ToolCall(data) => {
                check_id(&data.id)?;
                check_text(&data.name, MAX_LABEL_BYTES, false)?;
                check_text(&data.arguments, MAX_TEXT_BYTES, true)?;
                check_optional_text(&data.output, MAX_TEXT_BYTES)?;
            }
        }
    }
    Ok(())
}

fn validate_approval(approval: &ApprovalRequest) -> Result<(), ProtocolError> {
    check_id(&approval.id)?;
    check_text(&approval.summary, 2_048, false)
}

fn validate_error(error: &ApiError) -> Result<(), ProtocolError> {
    check_id(&error.code)?;
    check_text(&error.message, 2_048, false)
}

fn check_optional_text(value: &Option<String>, max_bytes: usize) -> Result<(), ProtocolError> {
    if let Some(value) = value {
        check_text(value, max_bytes, true)?;
    }
    Ok(())
}

fn check_count(count: usize) -> Result<(), ProtocolError> {
    check_limit(count, MAX_ITEMS)
}

fn check_limit(count: usize, max: usize) -> Result<(), ProtocolError> {
    if count > max {
        Err(ProtocolError::InvalidMessage)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Component, Path};

    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use p256::ecdsa::{Signature, VerifyingKey, signature::hazmat::PrehashVerifier};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn update_hashes_are_strict_sha256_and_unknown_modes_are_rejected() {
        assert!(check_binary_hash(&"a".repeat(64)).is_ok());
        for invalid in [
            "".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "g".repeat(64),
            "é".repeat(32),
        ] {
            assert!(check_binary_hash(&invalid).is_err());
        }
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../fixtures/v2/prepare-update-request.json"
        ))
        .unwrap();
        assert!(decode_client_message(&serde_json::to_vec(&value).unwrap()).is_ok());
        value["request"]["params"]["mode"] = "automatic".into();
        assert!(decode_client_message(&serde_json::to_vec(&value).unwrap()).is_err());
        value["request"]["params"]["mode"] = "now".into();
        value["request"]["params"]["targetHash"] = "not-a-hash".into();
        assert!(decode_client_message(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn v2_request_response_and_event_round_trip() {
        let hello = ClientMessage::Hello(ClientHello {
            message_type: HelloMessageType::Hello,
            supported_protocol_versions: vec![PROTOCOL_VERSION],
            client_version: "0.1.0-dev".into(),
        });
        let bytes = encode_client_message(&hello).unwrap();
        assert_eq!(decode_client_message(&bytes).unwrap(), hello);

        let request = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "request-1".into(),
            request: Request::CreateSession(CreateSessionParams {
                session_id: "session-1".into(),
                project_id: "project-1".into(),
                runtime_id: "pi".into(),
                expected_cwd_fence: "cwd-1".into(),
                start: SessionStart::New,
            }),
        });
        let bytes = encode_client_message(&request).unwrap();
        assert_eq!(decode_client_message(&bytes).unwrap(), request);

        let response = ServerMessage::Response(ServerResponse {
            message_type: ResponseMessageType::Response,
            protocol_version: PROTOCOL_VERSION,
            request_id: "request-1".into(),
            result: RequestResult::Unknown(ErrorResult {
                error: ApiError {
                    code: "OUTCOME_UNKNOWN".into(),
                    message: "reconcile the session; do not retry".into(),
                },
            }),
        });
        let bytes = encode_server_message(&response).unwrap();
        assert_eq!(decode_server_message(&bytes).unwrap(), response);

        let event = ServerMessage::Event(ServerEvent {
            message_type: EventMessageType::Event,
            protocol_version: PROTOCOL_VERSION,
            event_stream_id: "daemon-stream-1".into(),
            sequence: 1,
            event: Event::SessionRemoved(SessionRemovedData {
                session_id: "session-1".into(),
            }),
        });
        let bytes = encode_server_message(&event).unwrap();
        assert_eq!(decode_server_message(&bytes).unwrap(), event);
    }

    #[test]
    fn v2_json_samples_are_valid_rust_and_serialize_back_unchanged() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/v2");
        for file in [
            "client-hello.json",
            "client-request.json",
            "create-session-request.json",
            "get-project-root-request.json",
            "open-session-request.json",
            "close-session-request.json",
            "approval-request.json",
            "set-model-request.json",
            "set-thinking-request.json",
            "subscribe-events-request.json",
            "subscribe-snapshot-request.json",
            "prepare-update-request.json",
            "cancel-update-request.json",
            "shutdown-request.json",
        ] {
            let bytes = std::fs::read(dir.join(file)).unwrap();
            let client = decode_client_message(&bytes).unwrap();
            assert_eq!(
                serde_json::to_value(&client).unwrap(),
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                "fixture {file}"
            );
        }

        for file in [
            "server-welcome.json",
            "runtimes-response.json",
            "session-tool-approval-event.json",
            "version-mismatch.json",
            "server-response.json",
            "unconfirmed-health-response.json",
            "migration-failed-health-response.json",
            "schema-migration-failed-response.json",
            "project-response.json",
            "project-root-response.json",
            "project-local-only.json",
            "project-stale-fence.json",
            "shutdown-response.json",
            "close-session-dispatched.json",
            "sessions-response.json",
            "session-response.json",
            "subscription-response.json",
            "resumed-subscription-response.json",
            "server-event.json",
            "message-changed-event.json",
            "approval-resolved-event.json",
            "unknown-response.json",
            "update-response.json",
            "update-changed-event.json",
        ] {
            let bytes = std::fs::read(dir.join(file)).unwrap();
            let server = decode_server_message(&bytes).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(
                    &encode_server_message(&server).unwrap()
                )
                .unwrap(),
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                "fixture {file}"
            );
        }
    }

    #[test]
    fn project_fixtures_keep_shared_data_path_free_and_mutations_fenced() {
        let requests: Vec<serde_json::Value> =
            serde_json::from_slice(include_bytes!("../../../fixtures/v2/project-requests.json"))
                .unwrap();
        assert_eq!(requests.len(), 5);
        for value in &requests {
            let bytes = serde_json::to_vec(value).unwrap();
            let request = decode_client_message(&bytes).unwrap();
            assert_eq!(serde_json::to_value(&request).unwrap(), *value);
            let mut invalid = value.clone();
            invalid["request"]["params"]["access"] = "local".into();
            assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        for root in [
            "".to_owned(),
            "a\0b".into(),
            "a\nb".into(),
            "a".repeat(MAX_TEXT_BYTES + 1),
        ] {
            let mut invalid = requests[0].clone();
            invalid["request"]["params"]["root"] = root.into();
            assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        for name in ["".to_owned(), "a\nb".into(), "a".repeat(MAX_ID_BYTES + 1)] {
            let mut invalid = requests[0].clone();
            invalid["request"]["params"]["name"] = name.into();
            assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut default_name = requests[0].clone();
        default_name["request"]["params"]
            .as_object_mut()
            .unwrap()
            .remove("name");
        assert!(decode_client_message(&serde_json::to_vec(&default_name).unwrap()).is_ok());
        for index in [2, 3, 4] {
            let mut invalid = requests[index].clone();
            invalid["request"]["params"]
                .as_object_mut()
                .unwrap()
                .remove("expectedCwdFence");
            assert!(decode_client_message(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let events: Vec<serde_json::Value> =
            serde_json::from_slice(include_bytes!("../../../fixtures/v2/project-events.json"))
                .unwrap();
        assert_eq!(events.len(), 3);
        for (index, value) in events.iter().enumerate() {
            let frame = decode_server_message(&serde_json::to_vec(value).unwrap()).unwrap();
            assert_eq!(serde_json::to_value(frame).unwrap(), *value);
            assert_eq!(value["sequence"], index + 1);
        }
        let mut response: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../fixtures/v2/project-response.json"))
                .unwrap();
        response["result"]["data"]["result"]["data"]["root"] = "/private/root".into();
        assert!(decode_server_message(&serde_json::to_vec(&response).unwrap()).is_err());
    }

    #[test]
    fn lifecycle_fixture_round_trips_and_stop_reasons_require_stopped() {
        let trace: Vec<serde_json::Value> = serde_json::from_slice(include_bytes!(
            "../../../fixtures/v2/session-lifecycle.json"
        ))
        .unwrap();
        assert_eq!(trace.len(), 15);
        assert_eq!(
            decode_server_message(include_bytes!(
                "../../../fixtures/v2/session-lifecycle.json"
            )),
            Err(ProtocolError::InvalidFrame)
        );
        for value in &trace {
            let bytes = serde_json::to_vec(value).unwrap();
            let event = decode_server_message(&bytes).unwrap();
            assert_eq!(serde_json::to_value(&event).unwrap(), *value);
        }
        for index in [5, 11, 14] {
            assert_eq!(
                trace[index]["event"]["data"]["session"]["state"]["state"],
                "stopped"
            );
            assert!(
                trace[index]["event"]["data"]["session"]
                    .get("activeRunId")
                    .is_none()
            );
        }
        let mut wrong_state = trace[0].clone();
        wrong_state["event"]["data"]["session"]["stopReason"] = "SESSION_CLOSED".into();
        assert_eq!(
            decode_server_message(&serde_json::to_vec(&wrong_state).unwrap()),
            Err(ProtocolError::InvalidMessage)
        );
        let mut unsafe_reason = trace[5].clone();
        unsafe_reason["event"]["data"]["session"]["stopReason"] = "arbitrary exception text".into();
        assert_eq!(
            decode_server_message(&serde_json::to_vec(&unsafe_reason).unwrap()),
            Err(ProtocolError::InvalidFrame)
        );
    }

    #[test]
    fn approval_decisions_and_capabilities_are_explicit() {
        let request = include_bytes!("../../../fixtures/v2/approval-request.json");
        let event = include_bytes!("../../../fixtures/v2/approval-resolved-event.json");
        for (wire, decision) in [
            ("allow_once", ApprovalDecision::AllowOnce),
            ("allow_for_session", ApprovalDecision::AllowForSession),
            ("deny", ApprovalDecision::Deny),
        ] {
            assert_eq!(serde_json::to_value(&decision).unwrap(), wire);
            let mut client: serde_json::Value = serde_json::from_slice(request).unwrap();
            client["request"]["params"]["decision"] = wire.into();
            let message = decode_client_message(&serde_json::to_vec(&client).unwrap()).unwrap();
            assert_eq!(
                decode_client_message(&encode_client_message(&message).unwrap()).unwrap(),
                message
            );

            let mut server: serde_json::Value = serde_json::from_slice(event).unwrap();
            server["event"]["data"]["decision"] = wire.into();
            let message = decode_server_message(&serde_json::to_vec(&server).unwrap()).unwrap();
            assert_eq!(
                decode_server_message(&encode_server_message(&message).unwrap()).unwrap(),
                message
            );
        }
        let mut client: serde_json::Value = serde_json::from_slice(request).unwrap();
        client["request"]["params"]["decision"] = "allow_always".into();
        assert_eq!(
            decode_client_message(&serde_json::to_vec(&client).unwrap()),
            Err(ProtocolError::InvalidFrame)
        );

        let mut session: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../fixtures/v2/server-event.json"))
                .unwrap();
        for supported in [false, true] {
            session["event"]["data"]["session"]["capabilities"]["approvalForSession"] =
                supported.into();
            assert!(decode_server_message(&serde_json::to_vec(&session).unwrap()).is_ok());
        }
        session["event"]["data"]["session"]["capabilities"]
            .as_object_mut()
            .unwrap()
            .remove("approvalForSession");
        assert_eq!(
            decode_server_message(&serde_json::to_vec(&session).unwrap()),
            Err(ProtocolError::InvalidFrame)
        );
    }

    #[test]
    fn decoder_rejects_oversize_unknown_fields_bad_versions_and_ids() {
        let too_large = vec![b' '; MAX_FRAME_BYTES + 1];
        assert_eq!(
            decode_client_message(&too_large),
            Err(ProtocolError::FrameTooLarge)
        );
        assert_eq!(decode_client_message(&[]), Err(ProtocolError::InvalidFrame));
        assert_eq!(
            decode_client_message(br#"{"type":"request"} {}"#),
            Err(ProtocolError::InvalidFrame)
        );
        assert_eq!(
            decode_client_message(br#"[{"type":"request"}]"#),
            Err(ProtocolError::InvalidFrame)
        );
        assert_eq!(
            decode_client_message(&[b'{', 0xff, b'}']),
            Err(ProtocolError::InvalidFrame)
        );

        let unknown_field = br#"{"type":"request","protocolVersion":9,"requestId":"r","request":{"method":"get_status","path":"C:/"}}"#;
        assert_eq!(
            decode_client_message(unknown_field),
            Err(ProtocolError::InvalidFrame)
        );

        let unknown_params = br#"{"type":"request","protocolVersion":9,"requestId":"r","request":{"method":"prompt","params":{"sessionId":"s","expectedStreamId":"stream","expectedCwdFence":"cwd","content":"hi","extra":true}}}"#;
        assert_eq!(
            decode_client_message(unknown_params),
            Err(ProtocolError::InvalidFrame)
        );

        let duplicate_field = br#"{"type":"request","protocolVersion":9,"requestId":"r","requestId":"other","request":{"method":"get_status"}}"#;
        assert_eq!(
            decode_client_message(duplicate_field),
            Err(ProtocolError::InvalidFrame)
        );

        let unsupported = br#"{"type":"request","protocolVersion":1,"requestId":"r","request":{"method":"get_status"}}"#;
        assert_eq!(
            decode_client_message(unsupported),
            Err(ProtocolError::UnsupportedVersion(1))
        );

        let empty_versions =
            br#"{"type":"hello","supportedProtocolVersions":[],"clientVersion":"test"}"#;
        assert_eq!(
            decode_client_message(empty_versions),
            Err(ProtocolError::InvalidMessage)
        );

        let duplicate_versions =
            br#"{"type":"hello","supportedProtocolVersions":[2,2],"clientVersion":"test"}"#;
        assert_eq!(
            decode_client_message(duplicate_versions),
            Err(ProtocolError::InvalidMessage)
        );

        let version_mismatch =
            br#"{"type":"version_mismatch","daemonVersion":"0.1.0","supportedProtocolVersions":[1]}"#;
        assert!(matches!(
            decode_server_message(version_mismatch),
            Ok(ServerMessage::VersionMismatch(_))
        ));

        let empty_id = br#"{"type":"request","protocolVersion":9,"requestId":"","request":{"method":"get_status"}}"#;
        assert_eq!(
            decode_client_message(empty_id),
            Err(ProtocolError::InvalidMessage)
        );
    }

    #[test]
    fn snapshot_required_subscriptions_must_carry_their_snapshot() {
        let missing_snapshot = br#"{"type":"response","protocolVersion":9,"requestId":"r","result":{"status":"applied","data":{"result":{"kind":"subscription","data":{"eventStreamId":"stream","sequence":0,"mode":"snapshot_required"}}}}}"#;
        assert_eq!(
            decode_server_message(missing_snapshot),
            Err(ProtocolError::InvalidMessage)
        );
    }

    #[test]
    fn rejects_oversized_prompts_before_encoding() {
        let message = ClientMessage::Request(ClientRequest {
            message_type: RequestMessageType::Request,
            protocol_version: PROTOCOL_VERSION,
            request_id: "request-1".into(),
            request: Request::Prompt(PromptParams {
                session_id: "session-1".into(),
                expected_stream_id: "stream-1".into(),
                expected_cwd_fence: "cwd-1".into(),
                content: "x".repeat(MAX_TEXT_BYTES + 1),
                delivery: DeliveryMode::FollowUp,
            }),
        });
        assert_eq!(
            encode_client_message(&message),
            Err(ProtocolError::InvalidMessage)
        );
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GoRoomIdentity {
        version: u16,
        private_key: String,
        salt: String,
        verifier: String,
        revision: u32,
    }

    #[test]
    fn go_room_identity_fixture_keeps_its_v0_shape() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/v0-go/room-identity.json");
        let fixture: GoRoomIdentity =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(fixture.version, 1);
        assert_eq!(fixture.revision, 1);

        let private_key = URL_SAFE_NO_PAD.decode(fixture.private_key).unwrap();
        let secret = p256::SecretKey::from_sec1_der(&private_key).unwrap();
        let public = p256::PublicKey::from(&secret);
        let point = public.to_sec1_bytes();
        assert_eq!(point.len(), 65);
        assert_eq!(URL_SAFE_NO_PAD.encode(point).len(), 87);

        let salt = URL_SAFE_NO_PAD.decode(fixture.salt).unwrap();
        let expected = URL_SAFE_NO_PAD.decode(fixture.verifier).unwrap();
        assert_eq!(salt.len(), 16);
        assert_eq!(expected.len(), 32);
        let mut actual = [0; 32];
        pbkdf2::pbkdf2_hmac::<pbkdf2::sha2::Sha256>(
            b"fixture-only-123",
            &salt,
            600_000,
            &mut actual,
        );
        assert_eq!(actual.as_slice(), expected);
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GoRoomSignature {
        source: String,
        public_key: String,
        parts: Vec<String>,
        signature: String,
    }

    #[test]
    fn go_room_signature_fixture_verifies_its_transcript() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/v0-go/room-signature.json");
        let fixture: GoRoomSignature =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(
            fixture
                .source
                .contains("67237383fef260a8111a14910bc97a56195de349")
        );

        let identity_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/v0-go/room-identity.json");
        let identity: GoRoomIdentity =
            serde_json::from_slice(&std::fs::read(identity_path).unwrap()).unwrap();
        let private_key = URL_SAFE_NO_PAD.decode(identity.private_key).unwrap();
        let secret = p256::SecretKey::from_sec1_der(&private_key).unwrap();
        let identity_public_key = p256::PublicKey::from(&secret).to_sec1_bytes();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(identity_public_key),
            fixture.public_key
        );

        let public_key = URL_SAFE_NO_PAD.decode(&fixture.public_key).unwrap();
        let verifying_key = VerifyingKey::from_sec1_bytes(&public_key).unwrap();
        let signature_bytes = URL_SAFE_NO_PAD.decode(&fixture.signature).unwrap();
        let signature = Signature::from_slice(&signature_bytes).unwrap();
        let transcript = serde_json::to_vec(&fixture.parts).unwrap();
        let digest = Sha256::digest(&transcript);
        verifying_key.verify_prehash(&digest, &signature).unwrap();

        let mut tampered_parts = fixture.parts;
        tampered_parts[2].push_str("-tampered");
        let tampered_digest = Sha256::digest(serde_json::to_vec(&tampered_parts).unwrap());
        assert!(
            verifying_key
                .verify_prehash(&tampered_digest, &signature)
                .is_err()
        );
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GoFixtureManifest {
        source_repository: String,
        files: Vec<GoFixtureEntry>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GoFixtureEntry {
        path: String,
        source_commit: String,
        source_path: String,
        source_blob: Option<String>,
        sha256: String,
    }

    #[test]
    fn v0_go_corpus_matches_its_pinned_manifest() {
        const SOURCE_COMMIT: &str = "67237383fef260a8111a14910bc97a56195de349";
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/v0-go");
        let manifest: GoFixtureManifest =
            serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest.source_repository,
            "github.com/cafecodework/pi-packages/packages/pi-cafe-space"
        );
        assert_eq!(manifest.files.len(), 38);
        assert_eq!(
            manifest
                .files
                .iter()
                .filter(|entry| entry.source_blob.is_some())
                .count(),
            36
        );

        for entry in manifest.files {
            assert_eq!(entry.source_commit, SOURCE_COMMIT, "{}", entry.path);
            assert!(!entry.source_path.is_empty(), "{}", entry.path);
            if entry.path.starts_with("protocol/") {
                let blob = entry
                    .source_blob
                    .as_ref()
                    .expect("copied source needs a Git blob ID");
                assert_eq!(blob.len(), 40);
                assert!(blob.bytes().all(|byte| byte.is_ascii_hexdigit()));
                assert_eq!(entry.source_path, entry.path);
            } else {
                assert!(
                    entry.source_blob.is_none(),
                    "recorded output is not a source blob"
                );
            }
            let relative = Path::new(&entry.path);
            assert!(
                relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
                "manifest path escapes fixture directory: {}",
                entry.path
            );
            let bytes = std::fs::read(root.join(relative)).unwrap();
            let actual_hash: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(
                actual_hash, entry.sha256,
                "fixture hash mismatch: {}",
                entry.path
            );
        }
    }
}
