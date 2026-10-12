use rusqlite::params;
use crate::{contracts::ProjectId, error::{CoreError, Result}};
use super::{ConversationService, ConversationSummary, parse_uuid};
use crate::contracts::ConversationId;

impl ConversationService {
    /// Literal, project-scoped lexical search. No raw LIKE wildcards or cross-
    /// project ranking; archived conversations are opt-in, deleted never shown.
    pub async fn search(&self, project: ProjectId, query: String, include_archived: bool, limit: usize) -> Result<Vec<ConversationSummary>> {
        if query.len() > 512 { return Err(CoreError::new("invalid_query", "Search text exceeds 512 bytes")); }
        let pattern = format!("%{}%", query.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        self.store.with_connection(move |conn| {
            let mut stmt = conn.prepare("SELECT c.id,c.title,c.archived_at IS NOT NULL,c.created_at FROM conversations c WHERE c.project_id=?1 AND c.deleted_at IS NULL AND (?2 OR c.archived_at IS NULL) AND (c.title LIKE ?3 ESCAPE '\\' OR EXISTS(SELECT 1 FROM branches b JOIN branch_messages bm ON bm.branch_id=b.id JOIN messages m ON m.id=bm.message_id WHERE b.conversation_id=c.id AND m.content LIKE ?3 ESCAPE '\\')) ORDER BY c.created_at DESC,c.id LIMIT ?4")?;
            let mut rows = stmt.query(params![project.to_string(),include_archived,pattern,limit.clamp(1,100) as i64])?;
            let mut result = vec![];
            while let Some(row) = rows.next()? { result.push(ConversationSummary { id: ConversationId(parse_uuid(&row.get::<_,String>(0)?)?), title: row.get(1)?, archived: row.get(2)?, created_at: row.get(3)? }); }
            Ok(result)
        }).await
    }
}
