use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use ts_rs::TS;
use uuid::Uuid;
use crate::error::{CoreError, Result};
use crate::conversations::{ExportFormat, RunResolution};
use crate::store::imports::ImportSource;

macro_rules! uuid_id {
    ($($name:ident),* $(,)?) => { $(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
        impl $name { pub fn new() -> Self { Self(Uuid::new_v4()) } }
        impl Default for $name { fn default() -> Self { Self::new() } }
        impl std::fmt::Display for $name { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) } }
    )* };
}
uuid_id!(ProjectId, ConversationId, BranchId, MessageId, JobId, NodeId, RunId, ActionId, GrantId, ArtifactId, DecisionId);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema, TS)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct Digest(#[schemars(regex(pattern = "^[0-9a-f]{64}$"))] String);
impl Digest {
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(CoreError::new("invalid_digest", "digest must be lowercase SHA-256 hex"));
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind { Omp, Codex, Opencode, Claude, Hermes, Alphacode }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState { Unknown, Unavailable, Unqualified, Qualified }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CapabilityObservation { pub name: String, pub state: CapabilityState, pub implementation_version: String, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub evidence_digest: Option<Digest>, #[ts(type = "number")] pub observed_at: i64 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct RoutePin { pub connection_id: String, pub model_id: String, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub harness: Option<HarnessKind> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SpendPolicy { FreeOnly, PaidAllowed }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ProjectPolicy {
    #[ts(type = "number")] pub version: u64, pub project_id: ProjectId,
    pub allowed_connection_ids: Vec<String>, pub allowed_tools: Vec<String>,
    pub read_roots: Vec<String>, pub write_roots: Vec<String>, pub network_destinations: Vec<String>,
    pub local_only: bool, pub max_workers: u16, pub max_requests: u32,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "number")] pub max_cost_microusd: Option<u64>,
    pub spend_policy: SpendPolicy, pub allow_unknown_cost: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunState { Queued, Ready, Running, AwaitingApproval, Blocked, Reviewing, Verifying, Paused, Cancelling, Cancelled, Succeeded, Failed }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ActionSpec {
    pub id: ActionId, pub run_id: RunId, pub project_id: ProjectId, pub kind: String,
    #[ts(type = "unknown")] pub canonical_args: serde_json::Value, #[ts(type = "unknown")] pub scope: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub destination: Option<String>,
    #[ts(type = "number")] pub policy_version: u64, pub expected_state_digest: Digest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ApprovalGrant {
    pub id: GrantId, pub action_id: ActionId, pub action_digest: Digest,
    #[ts(type = "number")] pub policy_version: u64, pub expected_state_digest: Digest,
    #[ts(type = "number")] pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "number")] pub revoked_at: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SessionRef { pub harness: HarnessKind, pub native_id: String, pub native_version: String, pub project_id: ProjectId, pub cwd: String, pub resolved_route: RoutePin }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CitationLocator {
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub section: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub line_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub line_end: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub sheet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub cell_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub bounding_box: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "number")] pub time_start_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "number")] pub time_end_ms: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Citation { pub source_id: String, pub revision_digest: Digest, pub locator: CitationLocator }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CitedExcerpt { pub text: String, pub citation: Citation, pub project_id: ProjectId, pub disclosure: DisclosurePolicy }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ContextPackage { pub digest: Digest, pub brief: String, pub constraints: Vec<String>, pub excerpts: Vec<CitedExcerpt>, pub artifacts: Vec<ArtifactId>, pub unresolved_questions: Vec<String>, pub originating_sessions: Vec<SessionRef> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CheckReceipt { pub name: String, pub passed: bool, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub evidence_digest: Option<Digest>, pub details: String }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ResultManifest { pub run_id: RunId, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub native_session: Option<SessionRef>, pub artifacts: Vec<ArtifactId>, pub checks: Vec<CheckReceipt>, pub acceptance_satisfied: bool, pub unresolved: Vec<String> }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentPart { Text { text: String }, Image { uri: String, mime_type: String }, Audio { uri: String, mime_type: String }, Artifact { artifact_id: ArtifactId } }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ToolDescriptor { pub name: String, pub description: String, #[ts(type = "unknown")] pub input_schema: serde_json::Value }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionInput { pub run_id: RunId, pub project_id: ProjectId, pub cwd: String, pub route: RoutePin, pub context: ContextPackage, pub policy: ProjectPolicy, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub resume: Option<SessionRef> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct TurnInput { pub request_id: Uuid, pub message_id: MessageId, pub content: Vec<ContentPart>, pub allowed_tools: Vec<ToolDescriptor>, pub budget: ProjectPolicy }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision { Allow, Deny }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PermissionAnswer { pub native_request_id: String, pub action: ActionSpec, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub grant: Option<ApprovalGrant>, pub decision: PermissionDecision }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HarnessDescriptor { pub harness: HarnessKind, pub version: String, pub transport: String, pub route_compatibility: Vec<String>, pub approval: CapabilityState, pub cancel: CapabilityState, pub resume: CapabilityState, pub child_observability: CapabilityState, pub capabilities: Vec<CapabilityObservation> }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol { OpenaiCompletions, OpenaiResponses, Anthropic, Gemini, Ollama, LlamaCpp, Custom }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthRef { SecretService { service: String, account: String }, Bitwarden { item_id: String, field: String }, NativeSession { harness: HarnessKind, profile: String } }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Capability { Text, Tools, Vision, Audio, Embeddings, Transcription, Reasoning }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ModelDescriptor { pub id: String, pub context_limit: u32, pub output_limit: u32, pub capabilities: Vec<CapabilityObservation> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig { pub id: String, pub protocol: ProviderProtocol, pub endpoint: String, pub models: Vec<ModelDescriptor>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub auth_ref: Option<AuthRef>, pub allowed_projects: Vec<ProjectId> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ConnectionDescriptor { pub id: String, pub protocol: ProviderProtocol, pub endpoint: String, pub models: Vec<ModelDescriptor>, pub capabilities: Vec<CapabilityObservation>, pub availability: CapabilityState, pub auth_available: bool }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRole { System, User, Assistant, Tool }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ProviderToolCall { pub id: String, pub name: String, #[ts(type = "unknown")] pub arguments: serde_json::Value }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ProviderMessage { pub role: ProviderRole, pub content: Vec<ContentPart>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub tool_call_id: Option<String>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub tool_calls: Option<Vec<ProviderToolCall>>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "unknown")] pub sdk_content: Option<serde_json::Value>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub sdk_route: Option<RoutePin>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub tool_result_error: Option<bool> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ProviderTurnInput { pub run_id: RunId, pub request_id: Uuid, pub message_id: MessageId, pub route: RoutePin, pub messages: Vec<ProviderMessage>, pub required_capabilities: Vec<Capability>, pub tool_schemas: Vec<ToolDescriptor>, pub disclosure_digest: Digest, pub policy: ProjectPolicy, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub max_output_tokens: Option<u32> }
// Never persisted or exported through UI IPC. Only inherited worker stdio uses this DTO.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct WorkerProviderTurnInput { #[serde(flatten)] #[ts(flatten)] pub input: ProviderTurnInput, pub credential_lease_id: String, pub connection: ConnectionConfig }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct BillingObservation { pub state: BillingState, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "number")] pub cost_microusd: Option<u64> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum BillingState { Unknown, Reported }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource { Provider, Unknown }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct UsageObservation { #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub input_tokens: Option<u32>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub output_tokens: Option<u32>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional, type = "number")] pub cost_microusd: Option<u64>, pub billing: BillingObservation, pub source: UsageSource }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScoreEvidence { None {}, RawTokenLogProb { value: f64, token: String, token_position: u32 }, CalibratedSelectiveRisk { upper_error_bound: f64, dataset_digest: Digest, calibrator_digest: Digest } }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentState { Available, Invalid, Abstain, Unqualified, Unavailable }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct JudgmentInput { pub decision_id: DecisionId, pub domain: String, pub domain_version: String, pub candidates: Vec<String>, pub input_refs: Vec<String>, pub input_digest: Digest, pub criterion: String, pub route_policy_digest: Digest }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct JudgmentRecord { pub decision_id: DecisionId, pub domain: String, pub domain_version: String, pub candidates: Vec<String>, pub input_refs: Vec<String>, pub input_digest: Digest, pub criterion: String, pub route_policy_digest: Digest, pub model_identity: String, pub runtime_identity_digest: Digest, pub prompt_digest: Digest, pub candidate_codes: std::collections::BTreeMap<String, String>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub choice: Option<String>, pub state: JudgmentState, pub score: ScoreEvidence, #[ts(type = "number")] pub latency_ms: u64, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub qualification_digest: Option<Digest>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub reason: Option<String> }

// ── C4: state transitions, checkpoints, reconciliation ───────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointPhase { Planned, PreMutation, PostMutation, PostVerification, PreConsequential }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRecord { pub run_id: RunId, pub phase: CheckpointPhase, pub state_digest: Digest, #[ts(type = "unknown")] pub manifest: serde_json::Value, #[ts(type = "number")] pub created_at: i64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReport { pub run_id: RunId, pub run_state: RunState, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub recorded_pid: Option<u64>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub recorded_start_time: Option<i64>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub checkpoint: Option<CheckpointRecord>, #[ts(type = "unknown")] pub observed_workspace: serde_json::Value, pub pending_mutation: bool, pub uncertain_actions: Vec<String>, pub recommended: Vec<String> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDecision { AcceptObserved, ResumeReadOnly, ContinueFromCheckpoint, Abandon }

// ── P3: immutable source ingestion and real extraction ───────────────────────────
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceInput { File { path: String }, Attachment { artifact_id: ArtifactId }, Fetched { url: String, receipt: Digest } }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DisclosurePolicy { LocalOnly, AllowedConnections { connection_ids: Vec<String> }, ExplicitPublic }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SourceRevision { pub source_id: String, pub project_id: ProjectId, pub digest: Digest, pub mime: String, pub extractor_id: String, pub extractor_version: String, pub locators: usize, #[ts(type = "number")] pub created_at: i64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CitationView { Content { text: String }, Deleted, Missing }

// ── P4: filter-before-ranking retrieval and deletion semantics ────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeletionScope { Source, Project, All }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DeletionReceipt { pub source_id: String, pub chunks_removed: usize, pub vectors_removed: usize, pub revisions_removed: usize, #[ts(type = "number")] pub applied_at: i64 }

// ── P5: approved memory and optional versioned local embeddings ───────────────────
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct MemoryCandidate { pub project_id: ProjectId, pub text: String, pub provenance: String, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub input_refs: Option<Vec<String>> }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDecision { Approve, EditAndApprove, Reject }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord { pub id: String, pub project_id: ProjectId, pub text: String, pub state: String, pub provenance: String, #[ts(type = "number")] pub created_at: i64 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingModelIdentity { pub connection_id: String, pub model_id: String, pub version: String, #[ts(type = "number")] pub dimensions: u32 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingIndexReceipt { pub revision_id: String, pub model: EmbeddingModelIdentity, pub chunks_indexed: usize, #[ts(type = "number")] pub indexed_at: i64 }

// ── J2/J3: domain qualification and advisory judgments ────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum JudgmentDomain { TaskIntent, RetrievalRelevance, HandoffCompleteness, EvidenceNeeded, AllowedRouteChoice }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum QualificationState { Enabled, Blocked }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct QualificationRecord { pub domain: JudgmentDomain, pub model_identity: String, pub prompt_digest: Digest, pub state: QualificationState, pub upper_error_bound: f64, pub acceptance_coverage: f64, #[ts(type = "number")] pub holdout_size: u32, #[ts(type = "number")] pub evaluated_at: i64, pub calibrator_digest: Digest }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdvisoryOutcome { QualifiedChoice { choice: String, decision_id: DecisionId }, NeedsPlanner { reason: String }, Unavailable }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreEvent {
    TurnStarted { message_id: MessageId },
    TextDelta { message_id: MessageId, delta: String },
    TextReplace { message_id: MessageId, text: String },
    ReasoningDelta { message_id: MessageId, delta: String },
    ProviderToolCall { message_id: MessageId, call: ProviderToolCall },
    ProviderMessage { message_id: MessageId, #[ts(type = "unknown")] content: serde_json::Value, route: RoutePin },
    ToolStarted { action: ActionSpec },
    ToolFinished { action_id: ActionId, #[ts(type = "unknown")] result: serde_json::Value },
    ApprovalNeeded { action: ActionSpec },
    StateChanged { entity_id: String, state: String },
    SessionBound { session: SessionRef },
    ArtifactAdded { artifact_id: ArtifactId, digest: Digest },
    JudgmentRecorded { judgment: Box<JudgmentRecord> },
    UsageObserved { usage: UsageObservation },
    Error { code: String, message: String, #[ts(type = "unknown")] details: serde_json::Value },
    TurnFinished { message_id: MessageId, status: RunState, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] stop_reason: Option<String> },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct WorkerEvent { #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub session_id: Option<String>, pub turn_id: String, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub tool_id: Option<String>, pub run_id: RunId, pub payload: CoreEvent }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope { #[ts(type = "1")] #[schemars(range(min = 1, max = 1))] pub protocol: u8, #[ts(type = "number")] pub seq: u64, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub request_id: Option<Uuid>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub project_id: Option<ProjectId>, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] pub run_id: Option<RunId>, #[ts(type = "number")] pub timestamp: i64, pub payload: CoreEvent }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Project { pub id: ProjectId, pub name: String, pub root: String, #[ts(type = "number")] pub created_at: i64 }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
// `ConversationQueueTurn` carries a full optional `ContextPackage`, which makes this wire
// enum large. Boxing is not possible (ts-rs `#[ts(optional)]` needs a plain `Option`, and
// serde/TS must stay wire-compatible), so the size is accepted rather than hidden behind a
// second indirection layer.
#[allow(clippy::large_enum_variant)]
pub enum CoreCommand { Status {}, Events { #[ts(type = "number")] after: u64 }, ProjectList {}, ProjectCreate { name: String, root: String }, Doctor {}, Shutdown {},
    ArtifactRead { project_id: ProjectId, artifact_id: ArtifactId },
    ImportPreview { project_id: ProjectId, source: ImportSource },
    ImportApply { project_id: ProjectId, preview_digest: Digest },
    BackupCreate { destination: String },
    RetentionSweep {},
    ObjectCapacity { project_id: ProjectId },
    ConversationCreate { project_id: ProjectId },
    ConversationHistory { conversation_id: ConversationId },
    ConversationRename { conversation_id: ConversationId, title: String },
    ConversationSearch { project_id: ProjectId, query: String, include_archived: bool, #[ts(type = "number")] limit: u32 },
    ConversationExport { conversation_id: ConversationId, format: ExportFormat },
    ConversationEdit { message_id: MessageId, parts: Vec<ContentPart> },
    ConversationQueueTurn { conversation_id: ConversationId, branch_id: BranchId, parts: Vec<ContentPart>, pin: RoutePin, #[serde(skip_serializing_if = "Option::is_none")] #[ts(optional)] context: Option<ContextPackage> },
    ConversationRegenerate { message_id: MessageId, pin: RoutePin },
    ConversationArchive { conversation_id: ConversationId },
    ConversationDelete { conversation_id: ConversationId, resolution: RunResolution },
    RunInterrupt { run_id: RunId } }

/// Authenticated read of a CAS object over the control socket. The object bytes are
/// base64-encoded in the JSON reply so raw, digest-verified bytes can cross the
/// 1 MiB frame; the CLI decodes and streams them to stdout. Objects beyond the
/// recoverable IPC budget are refused with `artifact_too_large_for_ipc`, never truncated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ArtifactRead { pub digest: Digest, #[ts(type = "number")] pub bytes: u64, pub data: String }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreReply { Ok { #[ts(type = "unknown")] value: serde_json::Value }, Error { code: String, message: String, #[ts(type = "unknown")] details: serde_json::Value } }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
// `Request` embeds the full command by design (the frame IS the command envelope). Boxing
// would force a deref at every `ipc.rs` call site and is not warranted for a control-plane
// message that serde heap-allocates anyway.
#[allow(clippy::large_enum_variant)]
pub enum ClientFrame { Hello { #[ts(type = "1")] #[schemars(range(min = 1, max = 1))] protocol: u8, session_nonce: String }, Request { request_id: Uuid, command: CoreCommand }, Subscribe { #[ts(type = "number")] after_seq: u64 } }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame { Hello { #[ts(type = "1")] #[schemars(range(min = 1, max = 1))] protocol: u8 }, Reply { request_id: Uuid, reply: CoreReply }, Event { event: Box<EventEnvelope> }, Error { code: String, message: String, #[ts(type = "number")] after_seq: u64 } }

// Root schema excludes private native-worker credential lease DTOs.
#[derive(JsonSchema)]
pub struct ContractSchema { pub client: ClientFrame, pub server: ServerFrame, pub policy: ProjectPolicy, pub context: ContextPackage, pub result: ResultManifest, pub provider: ConnectionConfig, pub judgment_input: JudgmentInput, pub worker_event: WorkerEvent, pub open_session: OpenSessionInput, pub turn: TurnInput, pub permission: PermissionAnswer }
