use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;
use crate::{contracts::*, error::{CoreError, Result}, store::events::{now_ms, redacted_json}};
use super::{*, turns::{queue_tx, validate_parts}};

impl ConversationService {
    pub async fn history(&self, id: ConversationId) -> Result<ConversationHistory> {
        self.store.with_transaction(move |tx| history_tx(tx, id)).await
    }
    /// Only user edits are supported. Editing never rewinds tools or mutates
    /// existing rows, even when an assistant's partial output follows the user.
    pub async fn edit(&self, message: MessageId, parts: Vec<ContentPart>) -> Result<BranchId> {
        self.store.with_transaction(move |tx| {
            let (conversation, branch, position, role) = locate(tx, message)?;
            let project = ensure_visible(tx, conversation, true)?;
            if role != "user" { return Err(CoreError::new("unsupported_edit", "Only user messages may be edited")); }
            validate_parts(tx, project, &parts)?;
            let id = fork(tx, conversation, branch, message, position, "edit")?;
            insert_message(tx, id, position, MessageId::new(), "user", &parts, "complete")?;
            Ok(id)
        }).await
    }
    pub async fn regenerate(&self, message: MessageId, pin: RoutePin) -> Result<BranchId> {
        self.regenerate_turn(message, Uuid::new_v4(), pin).await.map(|t| t.branch_id)
    }
    /// IPC callers use their stable request ID; replay returns the same sibling.
    pub async fn regenerate_turn(&self, message: MessageId, request: Uuid, pin: RoutePin) -> Result<QueuedTurn> {
        let gate = self.context_gate.clone();
        self.store.with_transaction(move |tx| {
            let intent = serde_json::json!({"regenerate":message,"pin":pin});
            let fingerprint = turns::digest(&intent)?;
            if let Some(turn) = turns::existing(tx, request, fingerprint.as_str())? { return Ok(turn); }
            let (conversation, branch, position, role) = locate(tx, message)?;
            ensure_visible(tx, conversation, true)?;
            if role != "assistant" { return Err(CoreError::new("unsupported_regeneration", "Regenerate an assistant response")); }
            let previous: Option<(String, String)> = tx.query_row("SELECT m.id,m.role FROM branch_messages b JOIN messages m ON m.id=b.message_id WHERE b.branch_id=?1 AND b.position=?2", params![branch.to_string(),position-1], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (user, user_role) = previous.ok_or_else(|| CoreError::new("missing_user_message", "Response has no preceding user message"))?;
            if user_role != "user" { return Err(CoreError::new("missing_user_message", "Response has no preceding user message")); }
            let snapshot: String = tx.query_row("SELECT snapshot FROM conversation_turns WHERE assistant_message_id=?1", [message.to_string()], |r| r.get(0))?;
            let old: TurnSnapshot = serde_json::from_str(&snapshot)?;
            let new_branch = fork(tx, conversation, branch, message, position, "regenerate")?;
            queue_tx(tx, conversation, new_branch, request, vec![], pin, Some(old.context), Some(MessageId(parse_uuid(&user)?)), Some(fingerprint), gate.as_deref())
        }).await
    }
}
fn locate(tx: &Transaction<'_>, message: MessageId) -> Result<(ConversationId, BranchId, i64, String)> {
    // Use the origin branch, not an arbitrary inherited occurrence.
    let row: Option<(String,String,i64,String)> = tx.query_row("SELECT b.conversation_id,m.branch_id,bm.position,m.role FROM messages m JOIN branches b ON b.id=m.branch_id JOIN branch_messages bm ON bm.branch_id=m.branch_id AND bm.message_id=m.id WHERE m.id=?1", [message.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (conversation, branch, position, role) = row.ok_or_else(|| CoreError::new("message_missing", "Message not found"))?;
    Ok((ConversationId(parse_uuid(&conversation)?),BranchId(parse_uuid(&branch)?),position,role))
}
fn fork(tx: &Transaction<'_>, conversation: ConversationId, parent: BranchId, message: MessageId, cutoff: i64, operation: &str) -> Result<BranchId> {
    let mutable_prefix: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM branch_messages bm JOIN messages m ON m.id=bm.message_id WHERE bm.branch_id=?1 AND bm.position<?2 AND m.status IN ('queued','streaming','awaiting_acceptance'))", params![parent.to_string(),cutoff], |r| r.get(0))?;
    if mutable_prefix { return Err(CoreError::new("prefix_not_sealed", "Resolve earlier streamed messages before sharing an immutable prefix")); }
    let branch = BranchId::new();
    let provenance = BranchProvenance { operation: operation.into(), original_message_id: Some(message), preserved_run_ids: run_ids(tx, conversation)?, prior_effects_undone: false };
    tx.execute("INSERT INTO branches(id,conversation_id,parent_id,fork_message_id,provenance) VALUES(?1,?2,?3,?4,?5)", params![branch.to_string(),conversation.to_string(),parent.to_string(),message.to_string(),redacted_json(&provenance)?])?;
    tx.execute("INSERT INTO branch_messages(branch_id,position,message_id) SELECT ?1,position,message_id FROM branch_messages WHERE branch_id=?2 AND position<?3", params![branch.to_string(),parent.to_string(),cutoff])?;
    Ok(branch)
}
pub(crate) fn insert_message(tx: &Transaction<'_>, branch: BranchId, position: i64, id: MessageId, role: &str, parts: &[ContentPart], status: &str) -> Result<()> {
    tx.execute("INSERT INTO messages(id,branch_id,role,content,created_at,status) VALUES(?1,?2,?3,?4,?5,?6)", params![id.to_string(),branch.to_string(),role,redacted_json(&parts)?,now_ms(),status])?;
    tx.execute("INSERT INTO branch_messages(branch_id,position,message_id) VALUES(?1,?2,?3)", params![branch.to_string(),position,id.to_string()])?;
    Ok(())
}
pub(crate) fn messages_tx(tx: &Transaction<'_>, branch: BranchId) -> Result<Vec<MessageView>> {
    let mut stmt = tx.prepare("SELECT m.id,m.branch_id,m.role,m.content,m.status,m.created_at,t.run_id,t.snapshot FROM branch_messages b JOIN messages m ON m.id=b.message_id LEFT JOIN conversation_turns t ON (t.assistant_message_id=m.id OR t.user_message_id=m.id) AND t.branch_id=m.branch_id WHERE b.branch_id=?1 ORDER BY b.position")?;
    let mut rows = stmt.query([branch.to_string()])?;
    let mut output = vec![];
    while let Some(row) = rows.next()? {
        let id = MessageId(parse_uuid(&row.get::<_,String>(0)?)?);
        let run: Option<String> = row.get(6)?;
        let run = run.map(|r| parse_uuid(&r).map(RunId)).transpose()?;
        let snapshot: Option<String> = row.get(7)?;
        let receipts = if let Some(run) = run { receipts_tx(tx, run)? } else { vec![] };
        output.push(MessageView { id, origin_branch_id: BranchId(parse_uuid(&row.get::<_,String>(1)?)?), role: row.get(2)?, parts: serde_json::from_str(&row.get::<_,String>(3)?)?, status: row.get(4)?, created_at: row.get(5)?, run_id: run, snapshot: snapshot.map(|v| serde_json::from_str(&v)).transpose()?, receipts });
    }
    Ok(output)
}
fn receipts_tx(tx: &Transaction<'_>, run: RunId) -> Result<Vec<EventEnvelope>> {
    let mut stmt = tx.prepare("SELECT seq,request_id,project_id,timestamp,payload FROM events WHERE run_id=?1 ORDER BY seq")?;
    let mut rows = stmt.query([run.to_string()])?;
    let mut output = vec![];
    while let Some(row) = rows.next()? {
        let request: Option<String> = row.get(1)?;
        let project: Option<String> = row.get(2)?;
        output.push(EventEnvelope { protocol: 1, seq: row.get::<_,i64>(0)? as u64, request_id: request.map(|v| parse_uuid(&v)).transpose()?, project_id: project.map(|v| parse_uuid(&v).map(ProjectId)).transpose()?, run_id: Some(run), timestamp: row.get(3)?, payload: serde_json::from_str(&row.get::<_,String>(4)?)? });
    }
    Ok(output)
}
pub(crate) fn history_tx(tx: &Transaction<'_>, id: ConversationId) -> Result<ConversationHistory> {
    let project_id = ensure_visible(tx, id, false)?;
    let (title, archived): (String, bool) = tx.query_row("SELECT title,archived_at IS NOT NULL FROM conversations WHERE id=?1", [id.to_string()], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let mut stmt = tx.prepare("SELECT id,parent_id,provenance FROM branches WHERE conversation_id=?1 ORDER BY rowid")?;
    let mut rows = stmt.query([id.to_string()])?;
    let mut branches = vec![];
    while let Some(row) = rows.next()? {
        let branch = BranchId(parse_uuid(&row.get::<_,String>(0)?)?);
        let parent: Option<String> = row.get(1)?;
        branches.push(BranchView { id: branch, parent_id: parent.map(|v| parse_uuid(&v).map(BranchId)).transpose()?, provenance: serde_json::from_str(&row.get::<_,String>(2)?)?, messages: messages_tx(tx, branch)? });
    }
    Ok(ConversationHistory { id, project_id, title, archived, branches })
}
