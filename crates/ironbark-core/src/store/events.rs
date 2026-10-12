use std::sync::LazyLock;
use regex::Regex;
use rusqlite::{params, Transaction};
use serde_json::Value;
use crate::{contracts::*, error::{CoreError, Result}};

static CREDENTIAL_TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(authorization\s*[:=]\s*(?:bearer\s+|basic\s+)?|bearer\s+|(?:api[_-]?key|access[_-]?token|refresh[_-]?token|password|client[_-]?secret|credential[_-]?lease[_-]?id)\s*[:=]\s*[\"']?)[^\s\"',;}]+"#).expect("constant redaction regex"));
static URL_CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(https?://)[^\s/@]+:[^\s/@]+@").expect("constant URL regex"));
// Cookie values can contain spaces, semicolons and folded continuation lines.
// Redact the complete value, not only its first whitespace-delimited token.
static COOKIE_HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\b(?:set-cookie|cookie)[ \t]*:[ \t]*)[^\r\n]*(?:\r?\n[ \t]+[^\r\n]*)*").expect("constant cookie header regex"));

pub fn redact_text(text: &str) -> String {
    let text = COOKIE_HEADER.replace_all(text, "${1}[REDACTED]");
    let text = CREDENTIAL_TEXT.replace_all(&text, "${1}[REDACTED]");
    URL_CREDENTIAL.replace_all(&text, "${1}[REDACTED]@").into_owned()
}
pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                let key = key.to_ascii_lowercase().replace('-', "_");
                if matches!(key.as_str(), "authorization" | "proxy_authorization" | "cookie" | "set_cookie" | "api_key" | "apikey" | "access_token" | "refresh_token" | "token" | "password" | "client_secret" | "secret" | "credential" | "credentials" | "credential_lease_id" | "lease_material" | "private_key") || key.ends_with("_api_key") || key.ends_with("_token") || key.ends_with("_secret") || key.ends_with("_password") {
                    *value = Value::String("[REDACTED]".into());
                } else { redact_value(value); }
            }
        }
        Value::Array(items) => { for item in items { redact_value(item); } }
        Value::String(text) => { *text = redact_text(text); }
        _ => {}
    }
}

/// Serialize only after redaction; the unredacted value never reaches SQLite.
pub fn redacted_json<T: serde::Serialize>(value: &T) -> Result<String> {
    let mut value = serde_json::to_value(value)?;
    redact_value(&mut value);
    Ok(serde_json::to_string(&value)?)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis().min(i64::MAX as u128) as i64
}

pub fn append_tx(tx: &Transaction<'_>, request_id: Option<uuid::Uuid>, project_id: Option<ProjectId>, run_id: Option<RunId>, event: &CoreEvent) -> Result<EventEnvelope> {
    let payload = redacted_json(event)?;
    if payload.len() > crate::ipc::MAX_FRAME - 1024 {
        return Err(CoreError::new("frame_too_large", "event exceeds the recoverable IPC frame limit"));
    }
    let timestamp = now_ms();
    tx.execute("INSERT INTO events(protocol,request_id,project_id,run_id,timestamp,payload) VALUES(1,?1,?2,?3,?4,?5)", params![request_id.map(|v| v.to_string()), project_id.map(|v| v.to_string()), run_id.map(|v| v.to_string()), timestamp, payload])?;
    let seq = u64::try_from(tx.last_insert_rowid()).map_err(|_| CoreError::new("sequence_overflow", "invalid event sequence"))?;
    Ok(EventEnvelope { protocol: 1, seq, request_id, project_id, run_id, timestamp, payload: serde_json::from_str(&payload)? })
}

pub fn events_page(conn: &rusqlite::Connection, after: u64, limit: usize) -> Result<Vec<EventEnvelope>> {
    let after = i64::try_from(after).map_err(|_| CoreError::new("invalid_cursor", "cursor exceeds SQLite range"))?;
    let mut stmt = conn.prepare("SELECT seq,request_id,project_id,run_id,timestamp,payload FROM events WHERE seq>?1 ORDER BY seq ASC LIMIT ?2")?;
    let mut rows = stmt.query(params![after, limit as i64])?;
    let mut events = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next()? {
        let payload: String = row.get(5)?;
        // UUID/timestamp/key overhead is less than 512 bytes. Leave reply/frame
        // headroom; stop before this row and recover it on the next cursor.
        let size = payload.len().saturating_add(512);
        if !events.is_empty() && bytes.saturating_add(size) > crate::ipc::MAX_FRAME - 1024 { break; }
        bytes = bytes.saturating_add(size);
        let request: Option<String> = row.get(1)?;
        let project: Option<String> = row.get(2)?;
        let run: Option<String> = row.get(3)?;
        let uuid = |value: String| uuid::Uuid::parse_str(&value).map_err(|_| CoreError::new("storage_corrupt", "invalid persisted UUID"));
        events.push(EventEnvelope { protocol: 1, seq: row.get::<_, i64>(0)? as u64, request_id: request.map(&uuid).transpose()?, project_id: project.map(|v| uuid(v).map(ProjectId)).transpose()?, run_id: run.map(|v| uuid(v).map(RunId)).transpose()?, timestamp: row.get(4)?, payload: serde_json::from_str(&payload)? });
    }
    Ok(events)
}
