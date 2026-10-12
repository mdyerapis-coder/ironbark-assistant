use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use ts_rs::TS;
use std::fmt::Write;
use crate::{contracts::*, error::Result, store::events::{redact_value, redact_text}};
use super::{ConversationService, branches::history_tx, ConversationHistory};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat { Json, Markdown }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ConversationExport { pub conversation_id: ConversationId, pub format: ExportFormat, pub content: String }
impl ConversationService {
    /// Invoked only by an explicit owner IPC command, never an adapter event.
    /// Returns text; this leaf does not publish or choose a filesystem target.
    pub async fn export(&self, id: ConversationId, format: ExportFormat) -> Result<ConversationExport> {
        self.store.with_transaction(move |tx| {
            let history = history_tx(tx, id)?;
            let mut value = serde_json::to_value(history)?;
            redact_value(&mut value);
            let content = match format {
                ExportFormat::Json => serde_json::to_string_pretty(&value)?,
                ExportFormat::Markdown => markdown(&serde_json::from_value::<ConversationHistory>(value)?)?,
            };
            Ok(ConversationExport { conversation_id: id, format, content })
        }).await
    }
}
fn markdown(history: &ConversationHistory) -> Result<String> {
    let mut output = format!("# {}\n\nConversation: `{}` · Project: `{}`\n\n", history.title.replace('\n', " "), history.id, history.project_id);
    for branch in &history.branches {
        let _ = writeln!(output, "## Branch `{}`\n", branch.id);
        if let Some(parent) = branch.parent_id { let _ = writeln!(output, "Parent: `{parent}`\n"); }
        let _ = writeln!(output, "Operation: {}. Earlier tool effects were **not undone**.\n", branch.provenance.operation);
        for run in &branch.provenance.preserved_run_ids { let _ = writeln!(output, "Preserved prior-effect run: `{run}`"); }
        for message in &branch.messages {
            let _ = writeln!(output, "\n### {} — {} (`{}`)\n", message.role, message.status, message.id);
            for part in &message.parts {
                match part {
                    ContentPart::Text { text } => { let _ = writeln!(output, "{text}\n"); }
                    ContentPart::Artifact { artifact_id } => { let _ = writeln!(output, "Artifact receipt: `{artifact_id}`\n"); }
                    ContentPart::Image { uri, mime_type } | ContentPart::Audio { uri, mime_type } => { let _ = writeln!(output, "Media ({mime_type}): {}\n", redact_text(uri)); }
                }
            }
            if let Some(snapshot) = &message.snapshot {
                let _ = writeln!(output, "Pinned route: `{}/{}` · context `{}` · disclosure `{}`\n", snapshot.route.connection_id, snapshot.route.model_id, snapshot.context.digest.as_str(), snapshot.disclosure_digest.as_str());
                for excerpt in &snapshot.context.excerpts {
                    let _ = writeln!(output, "Citation: source `{}` · revision `{}` · locator `{}`\n\n{}\n", excerpt.citation.source_id, excerpt.citation.revision_digest.as_str(), serde_json::to_string(&excerpt.citation.locator)?, excerpt.text);
                }
            }
            // Include actual tool/result/error/usage receipts verbatim as
            // redacted JSON. Unknown billing remains unknown, not a $0 label.
            for receipt in &message.receipts {
                if matches!(receipt.payload, CoreEvent::ToolStarted { .. } | CoreEvent::ToolFinished { .. } | CoreEvent::ProviderToolCall { .. } | CoreEvent::Error { .. } | CoreEvent::UsageObserved { .. }) {
                    let _ = writeln!(output, "Receipt sequence {}:\n\n```json\n{}\n```\n", receipt.seq, serde_json::to_string_pretty(receipt)?);
                }
            }
        }
    }
    Ok(redact_text(&output))
}
