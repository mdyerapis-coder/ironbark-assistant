use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
use crate::{contracts::*, error::{CoreError, Result}, store::events::{append_tx, redact_value, redacted_json}};
use super::{*, branches::{insert_message, messages_tx}};

type TurnRow=(String,String,String,String,String,String,i64,String);

impl ConversationService {
    pub async fn start_turn(&self, conversation: ConversationId, branch: BranchId, request: Uuid, parts: Vec<ContentPart>, pin: RoutePin) -> Result<RunId> {
        self.queue_turn(conversation, branch, request, parts, pin, None).await.map(|t| t.run_id)
    }
    pub async fn queue_turn(&self, conversation: ConversationId, branch: BranchId, request: Uuid, parts: Vec<ContentPart>, pin: RoutePin, context: Option<ContextPackage>) -> Result<QueuedTurn> {
        let gate = self.context_gate.clone();
        self.store.with_transaction(move |tx| queue_tx(tx, conversation, branch, request, parts, pin, context, None, None, gate.as_deref())).await
    }
    /// The durable claim is exact dispatch ownership, not permission to skip
    /// P1 qualification/P4 disclosure/C4 policy checks. Claims never auto-reset.
    pub async fn claim_dispatch(&self, run: RunId) -> Result<Option<TurnDispatch>> {
        let gate = self.context_gate.clone();
        self.store.with_transaction(move |tx| {
            let row: Option<TurnRow> = tx.query_row("SELECT t.conversation_id,t.branch_id,t.request_id,t.user_message_id,t.assistant_message_id,t.snapshot,t.dispatch_claimed,r.state FROM conversation_turns t JOIN runs r ON r.id=t.run_id WHERE t.run_id=?1", [run.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
            let (conversation, branch, request, user, assistant, snapshot, claimed, state) = row.ok_or_else(|| CoreError::new("turn_missing", "Turn does not exist"))?;
            ensure_visible(tx, ConversationId(parse_uuid(&conversation)?), true)?;
            if claimed != 0 { return Ok(None); }
            if state != "queued" && state != "ready" { return Err(CoreError::new("dispatch_state_conflict", "Turn is not queued or ready")); }
            let branch_id = BranchId(parse_uuid(&branch)?);
            let snapshot: TurnSnapshot = serde_json::from_str(&snapshot)?;
            let current_policy = validate_snapshot(tx, snapshot.policy.project_id, &snapshot.route, &snapshot.context, gate.as_deref())?;
            if current_policy != snapshot.policy { return Err(CoreError::new("policy_snapshot_stale", "Pinned policy changed before dispatch; queue a new owner-approved turn")); }
            let mut messages = messages_tx(tx, branch_id)?;
            let assistant_message_id = MessageId(parse_uuid(&assistant)?);
            // Do not send the empty pending assistant to the provider.
            messages.retain(|m| m.id != assistant_message_id);
            for message in &messages {
                if !message.parts.is_empty() { validate_parts(tx, snapshot.policy.project_id, &message.parts)?; }
                if let Some(previous) = &message.snapshot { validate_snapshot(tx, snapshot.policy.project_id, &snapshot.route, &previous.context, gate.as_deref())?; }
            }
            for receipt in &snapshot.artifact_receipts {
                if artifact_receipt(tx, snapshot.policy.project_id, receipt.artifact_id)? != *receipt { return Err(CoreError::new("attachment_evidence_changed", "Pinned artifact digest/origin changed before dispatch")); }
            }
            tx.execute("UPDATE conversation_turns SET dispatch_claimed=1 WHERE run_id=?1 AND dispatch_claimed=0", [run.to_string()])?;
            Ok(Some(TurnDispatch { run_id: run, request_id: parse_uuid(&request)?, branch_id, user_message_id: MessageId(parse_uuid(&user)?), assistant_message_id, snapshot, messages }))
        }).await
    }
    /// Parent event-worker integration supplies an ordered per-run sequence.
    /// Persistence and projection are atomic; a retry never concatenates twice.
    pub async fn apply_worker_event(&self, run: RunId, sequence: u64, event: CoreEvent) -> Result<bool> {
        self.store.with_transaction(move |tx| {
            let (conversation, assistant, previous): (String,String,i64) = tx.query_row("SELECT conversation_id,assistant_message_id,last_worker_seq FROM conversation_turns WHERE run_id=?1", [run.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let project = ensure_visible(tx, ConversationId(parse_uuid(&conversation)?), false)?;
            let assistant = MessageId(parse_uuid(&assistant)?);
            let sequence = i64::try_from(sequence).map_err(|_| CoreError::new("invalid_sequence", "Worker sequence overflow"))?;
            if sequence <= previous { return Ok(false); }
            if sequence != previous + 1 { return Err(CoreError::new("worker_sequence_gap", "Recover missing events before projecting")); }
            let mut value = serde_json::to_value(event)?;
            redact_value(&mut value);
            let event: CoreEvent = serde_json::from_value(value)?;
            let status: String = tx.query_row("SELECT status FROM messages WHERE id=?1", [assistant.to_string()], |r| r.get(0))?;
            let is_text = matches!(&event, CoreEvent::TextDelta { .. } | CoreEvent::TextReplace { .. } | CoreEvent::ReasoningDelta { .. } | CoreEvent::ProviderToolCall { .. } | CoreEvent::ProviderMessage { .. });
            if is_text && !matches!(status.as_str(), "queued" | "streaming") { return Err(CoreError::new("turn_stream_closed", "Closed response is immutable")); }
            match &event {
                CoreEvent::TextDelta { message_id, delta } | CoreEvent::TextReplace { message_id, text: delta } => {
                    require_message(*message_id, assistant)?;
                    let content: String = tx.query_row("SELECT content FROM messages WHERE id=?1", [assistant.to_string()], |r| r.get(0))?;
                    let mut parts: Vec<ContentPart> = serde_json::from_str(&content)?;
                    if matches!(&event, CoreEvent::TextReplace { .. }) {
                        parts = vec![ContentPart::Text { text: delta.clone() }];
                    } else if let Some(ContentPart::Text { text }) = parts.last_mut() { text.push_str(delta); }
                    else { parts.push(ContentPart::Text { text: delta.clone() }); }
                    let content = redacted_json(&parts)?;
                    if content.len() > 4 * 1024 * 1024 { return Err(CoreError::new("message_too_large", "Response exceeds bounded projection size")); }
                    tx.execute("UPDATE messages SET content=?2,status='streaming' WHERE id=?1", params![assistant.to_string(),content])?;
                }
                CoreEvent::TurnStarted { message_id } => { require_message(*message_id, assistant)?; if status != "queued" { return Err(CoreError::new("turn_already_started", "Response is already started or closed")); } tx.execute("UPDATE messages SET status='streaming' WHERE id=?1", [assistant.to_string()])?; }
                CoreEvent::ReasoningDelta { message_id, .. } | CoreEvent::ProviderToolCall { message_id, .. } => require_message(*message_id, assistant)?,
                CoreEvent::ProviderMessage { message_id, route, .. } => {
                    require_message(*message_id, assistant)?;
                    let snapshot: String = tx.query_row("SELECT snapshot FROM conversation_turns WHERE run_id=?1", [run.to_string()], |r| r.get(0))?;
                    if serde_json::from_str::<TurnSnapshot>(&snapshot)?.route != *route { return Err(CoreError::new("route_changed", "Provider message does not match the pinned route")); }
                }
                CoreEvent::ToolStarted { action } | CoreEvent::ApprovalNeeded { action } => {
                    if action.run_id != run || action.project_id != project { return Err(CoreError::new("receipt_scope_mismatch", "Tool receipt belongs to another run/project")); }
                }
                CoreEvent::ToolFinished { action_id, .. } => {
                    let belongs: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM actions WHERE id=?1 AND run_id=?2)", params![action_id.to_string(),run.to_string()], |r| r.get(0))?;
                    if !belongs { return Err(CoreError::new("receipt_scope_mismatch", "Unknown action receipt")); }
                }
                CoreEvent::TurnFinished { message_id, status: finish, .. } => {
                    require_message(*message_id, assistant)?;
                    let projection = match finish { RunState::Succeeded => "awaiting_acceptance", RunState::Failed => "failed", RunState::Cancelled | RunState::Cancelling | RunState::Blocked | RunState::Paused => "interrupted", _ => return Err(CoreError::new("invalid_turn_finish", "Invalid worker stream finish state")) };
                    if !matches!(status.as_str(), "queued" | "streaming") && status != projection { return Err(CoreError::new("turn_stream_closed", "Closed response cannot acquire a different native completion state")); }
                    tx.execute("UPDATE messages SET status=?2 WHERE id=?1", params![assistant.to_string(),projection])?;
                    // Native completion is never C4 success/cancellation evidence.
                    tx.execute("UPDATE runs SET state='blocked' WHERE id=?1 AND state NOT IN ('succeeded','failed','cancelled')", [run.to_string()])?;
                }
                CoreEvent::Error { .. } => {
                    if !matches!(status.as_str(), "queued" | "streaming" | "failed") { return Err(CoreError::new("turn_stream_closed", "Closed response cannot be rewritten by a late error")); }
                    tx.execute("UPDATE messages SET status='failed' WHERE id=?1", [assistant.to_string()])?;
                    tx.execute("UPDATE runs SET state='blocked' WHERE id=?1 AND state NOT IN ('succeeded','failed','cancelled')", [run.to_string()])?;
                }
                // C4 lifecycle/state acceptance is not projected from an
                // untrusted worker StateChanged/native_done label.
                CoreEvent::StateChanged { .. } => return Err(CoreError::new("unsupported_worker_state", "C4 owns durable run state transitions")),
                CoreEvent::SessionBound { session } if session.project_id != project => { return Err(CoreError::new("session_scope_mismatch", "Native session belongs to another project")); }
                _ => {}
            }
            append_tx(tx, None, Some(project), Some(run), &event)?;
            tx.execute("UPDATE conversation_turns SET last_worker_seq=?2 WHERE run_id=?1", params![run.to_string(),sequence])?;
            Ok(true)
        }).await
    }
    /// Call only after C4 has committed its receipt-validated terminal state.
    pub async fn reconcile_projection(&self, run: RunId) -> Result<()> {
        self.store.with_transaction(move |tx| {
            let (state, assistant): (String,String) = tx.query_row("SELECT r.state,t.assistant_message_id FROM runs r JOIN conversation_turns t ON t.run_id=r.id WHERE r.id=?1", [run.to_string()], |r| Ok((r.get(0)?,r.get(1)?)))?;
            let status = match state.as_str() { "succeeded" => "complete", "failed" => "failed", "cancelled" => "interrupted", _ => return Err(CoreError::new("run_reconciliation_required", "No C4 terminal receipt has been committed")) };
            tx.execute("UPDATE messages SET status=?2 WHERE id=?1", params![assistant,status])?;
            Ok(())
        }).await
    }
}
fn require_message(actual: MessageId, expected: MessageId) -> Result<()> {
    if actual != expected { return Err(CoreError::new("message_scope_mismatch", "Event targets another message")); } Ok(())
}
pub(crate) fn digest<T: Serialize>(value: &T) -> Result<Digest> { Digest::parse(&format!("{:x}", Sha256::digest(serde_json::to_vec(value)?))) }
pub(crate) fn existing(tx: &Transaction<'_>, request: Uuid, fingerprint: &str) -> Result<Option<QueuedTurn>> {
    let row: Option<(String,String,String,String,String)> = tx.query_row("SELECT run_id,branch_id,user_message_id,assistant_message_id,request_digest FROM conversation_turns WHERE request_id=?1", [request.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    row.map(|(run, branch, user, assistant, old)| {
        if old != fingerprint { return Err(CoreError::new("request_id_conflict", "Request ID has different turn content or route")); }
        Ok(QueuedTurn { run_id: RunId(parse_uuid(&run)?), branch_id: BranchId(parse_uuid(&branch)?), user_message_id: MessageId(parse_uuid(&user)?), assistant_message_id: MessageId(parse_uuid(&assistant)?), replayed: true })
    }).transpose()
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn queue_tx(tx: &Transaction<'_>, conversation: ConversationId, branch: BranchId, request: Uuid, parts: Vec<ContentPart>, pin: RoutePin, context: Option<ContextPackage>, reuse_user: Option<MessageId>, fingerprint: Option<Digest>, gate: Option<&dyn ContextGate>) -> Result<QueuedTurn> {
    let fingerprint = match fingerprint { Some(v) => v, None => digest(&serde_json::json!({"conversation":conversation,"branch":branch,"parts":parts,"pin":pin,"context":context}))? };
    if let Some(turn) = existing(tx, request, fingerprint.as_str())? { return Ok(turn); }
    let project = ensure_visible(tx, conversation, true)?;
    let belongs: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM branches WHERE id=?1 AND conversation_id=?2)", params![branch.to_string(),conversation.to_string()], |r| r.get(0))?;
    if !belongs { return Err(CoreError::new("branch_scope_mismatch", "Branch does not belong to conversation")); }
    let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM conversation_turns t JOIN runs r ON r.id=t.run_id WHERE t.branch_id=?1 AND r.state NOT IN ('succeeded','failed','cancelled'))", [branch.to_string()], |r| r.get(0))?;
    if active { return Err(CoreError::new("branch_active", "Resolve the branch's earlier run before sending again")); }
    let context = match context {
        Some(context) => context,
        None => ContextPackage { digest: digest(&serde_json::json!({"brief":"","constraints":[],"excerpts":[],"artifacts":[],"unresolved_questions":[],"originating_sessions":[]}))?, brief: String::new(), constraints: vec![], excerpts: vec![], artifacts: vec![], unresolved_questions: vec![], originating_sessions: vec![] },
    };
    let context: ContextPackage = serde_json::from_str(&redacted_json(&context)?)?;
    let policy = validate_snapshot(tx, project, &pin, &context, gate)?;
    let mut parts: Vec<ContentPart> = serde_json::from_str(&redacted_json(&parts)?)?;
    let mut reuse_user = reuse_user;
    let prior_messages = messages_tx(tx, branch)?;
    if reuse_user.is_none()
        && let Some(last) = prior_messages.last().filter(|m| m.role == "user" && m.run_id.is_none()) {
            if last.parts != parts { return Err(CoreError::new("pending_edit_conflict", "Send must match the edited branch's existing user content")); }
            reuse_user = Some(last.id);
        }
    if let Some(user) = reuse_user {
        let previous = prior_messages.last().filter(|m| m.id == user && m.role == "user")
            .ok_or_else(|| CoreError::new("message_scope_mismatch", "Reused user must be the branch's final prompt"))?;
        parts = previous.parts.clone();
    }
    validate_parts(tx, project, &parts)?;
    for message in &prior_messages {
        if !message.parts.is_empty() { validate_parts(tx, project, &message.parts)?; }
        if let Some(snapshot) = &message.snapshot { validate_snapshot(tx, project, &pin, &snapshot.context, gate)?; }
    }
    let mut artifact_ids = context.artifacts.clone();
    for part in parts.iter().chain(prior_messages.iter().flat_map(|m| m.parts.iter())) {
        if let ContentPart::Artifact { artifact_id } = part { artifact_ids.push(*artifact_id); }
    }
    artifact_ids.sort_by_key(|id| id.to_string());
    artifact_ids.dedup();
    let artifact_receipts = artifact_ids.into_iter().map(|id| artifact_receipt(tx, project, id)).collect::<Result<Vec<_>>>()?;
    let disclosure_digest = digest(&serde_json::json!({"route":pin,"context":context,"policy":policy,"parts":parts,"reuse_user":reuse_user,"history":prior_messages,"artifact_receipts":artifact_receipts}))?;
    let snapshot = TurnSnapshot { route: pin, context, disclosure_digest, policy, artifact_receipts };
    let snapshot_json = redacted_json(&snapshot)?;
    let position: i64 = tx.query_row("SELECT COALESCE(MAX(position)+1,0) FROM branch_messages WHERE branch_id=?1", [branch.to_string()], |r| r.get(0))?;
    let user = match reuse_user { Some(id) => id, None => { let id = MessageId::new(); insert_message(tx,branch,position,id,"user",&parts,"complete")?; id } };
    let assistant = MessageId::new();
    insert_message(tx,branch,position + if reuse_user.is_some() {0} else {1},assistant,"assistant",&[],"queued")?;
    let job = JobId::new(); let node = NodeId::new(); let run = RunId::new();
    let body = redacted_json(&serde_json::json!({"kind":"conversation_turn","conversation_id":conversation,"branch_id":branch,"request_id":request,"snapshot":snapshot,"user_message_id":user,"assistant_message_id":assistant}))?;
    tx.execute("INSERT INTO jobs(id,project_id,body,state) VALUES(?1,?2,?3,'queued')", params![job.to_string(),project.to_string(),body])?;
    tx.execute("INSERT INTO nodes(id,job_id,body,state) VALUES(?1,?2,?3,'queued')", params![node.to_string(),job.to_string(),body])?;
    tx.execute("INSERT INTO runs(id,node_id,state,body) VALUES(?1,?2,'queued',?3)", params![run.to_string(),node.to_string(),body])?;
    tx.execute("INSERT INTO conversation_turns(run_id,conversation_id,branch_id,request_id,request_digest,user_message_id,assistant_message_id,snapshot) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![run.to_string(),conversation.to_string(),branch.to_string(),request.to_string(),fingerprint.as_str(),user.to_string(),assistant.to_string(),snapshot_json])?;
    append_tx(tx,None,Some(project),Some(run),&CoreEvent::StateChanged { entity_id: run.to_string(), state: "queued".into() })?;
    Ok(QueuedTurn { run_id: run, branch_id: branch, user_message_id: user, assistant_message_id: assistant, replayed: false })
}

pub(crate) fn validate_parts(tx: &Transaction<'_>, project: ProjectId, parts: &[ContentPart]) -> Result<()> {
    if parts.is_empty() || parts.len() > 128 || serde_json::to_vec(parts)?.len() > 512 * 1024 { return Err(CoreError::new("invalid_content", "Turn content must be nonempty and bounded")); }
    for part in parts {
        match part {
            ContentPart::Text { text } if text.trim().is_empty() => return Err(CoreError::new("invalid_content", "Text part is empty")),
            ContentPart::Artifact { artifact_id } => {
                artifact_receipt(tx, project, *artifact_id)?;
            }
            ContentPart::Image { .. } | ContentPart::Audio { .. } => return Err(CoreError::new("attachment_evidence_missing", "URI-only media lacks a canonical project-owned artifact receipt")),
            _ => {}
        }
    }
    Ok(())
}
fn artifact_receipt(tx: &Transaction<'_>, project: ProjectId, id: ArtifactId) -> Result<ArtifactReceipt> {
    let row: Option<(String,Option<String>)> = tx.query_row("SELECT digest,run_id FROM artifacts WHERE id=?1 AND project_id=?2", params![id.to_string(),project.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (hash, run) = row.ok_or_else(|| CoreError::new("attachment_evidence_missing", "Artifact has no project-owned durable provenance"))?;
    Ok(ArtifactReceipt { artifact_id: id, digest: Digest::parse(&hash)?, origin_run_id: run.map(|r| parse_uuid(&r).map(RunId)).transpose()? })
}
fn validate_snapshot(tx: &Transaction<'_>, project: ProjectId, pin: &RoutePin, context: &ContextPackage, gate: Option<&dyn ContextGate>) -> Result<ProjectPolicy> {
    let policy: String = tx.query_row("SELECT body FROM policies WHERE project_id=?1 ORDER BY version DESC LIMIT 1", [project.to_string()], |r| r.get(0))?;
    let policy: ProjectPolicy = serde_json::from_str(&policy)?;
    if policy.project_id != project || !policy.allowed_connection_ids.contains(&pin.connection_id) { return Err(CoreError::new("route_denied", "Pinned connection is not allowed by project policy")); }
    let config: Option<String> = tx.query_row("SELECT config FROM provider_references WHERE id=?1", [&pin.connection_id], |r| r.get(0)).optional()?;
    let config: ConnectionConfig = serde_json::from_str(&config.ok_or_else(|| CoreError::new("connection_missing", "Pinned connection is not registered"))?)?;
    if !config.allowed_projects.contains(&project) || !config.models.iter().any(|m| m.id == pin.model_id) { return Err(CoreError::new("route_denied", "Pinned model/project is not registered on this connection")); }
    let local = is_loopback(&config.endpoint);
    if policy.local_only && !local { return Err(CoreError::new("disclosure_denied", "Local-only project cannot use a nonloopback endpoint")); }
    if !local && gate.is_none() && (!context.artifacts.is_empty() || !context.originating_sessions.is_empty() || !context.excerpts.is_empty() || !context.brief.is_empty() || !context.constraints.is_empty() || !context.unresolved_questions.is_empty()) {
        return Err(CoreError::new("disclosure_evidence_missing", "Remote context requires P4's current native disclosure validator"));
    }
    for artifact in &context.artifacts { validate_parts(tx, project, &[ContentPart::Artifact { artifact_id: *artifact }])?; }
    for session in &context.originating_sessions { if session.project_id != project { return Err(CoreError::new("context_scope_mismatch", "Context session belongs to another project")); } }
    for excerpt in &context.excerpts {
        // Historic locator/text must be exact. Never invent citations or use the
        // latest source revision in place of the cited digest.
        let exact: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sources s JOIN source_revisions r ON r.source_id=s.id JOIN chunks c ON c.revision_id=r.id WHERE s.id=?1 AND s.project_id=?2 AND r.digest=?3 AND c.locator=?4 AND c.content=?5)", params![excerpt.citation.source_id,project.to_string(),excerpt.citation.revision_digest.as_str(),serde_json::to_string(&excerpt.citation.locator)?,excerpt.text], |r| r.get(0))?;
        if !exact { return Err(CoreError::new("citation_evidence_missing", "Exact historic citation/locator/text is unavailable")); }
    }
    if serde_json::to_vec(context)?.len() > 512 * 1024 { return Err(CoreError::new("context_too_large", "Context exceeds turn snapshot limit")); }
    if let Some(gate) = gate { gate.validate(tx, project, pin, &policy, context)?; }
    Ok(policy)
}
fn is_loopback(endpoint: &str) -> bool {
    let Some(rest) = endpoint.strip_prefix("http://").or_else(|| endpoint.strip_prefix("https://")) else { return false; };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') { return false; }
    let host = if authority.starts_with('[') { authority.split(']').next().unwrap_or("").trim_start_matches('[') } else { authority.split(':').next().unwrap_or("") };
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}
