#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError { pub code: String, pub message: String }
pub type Result<T> = std::result::Result<T, CoreError>;
impl CoreError { pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self { Self { code: code.into(), message: message.into() } } }
impl std::fmt::Display for CoreError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) } }
impl std::error::Error for CoreError {}
impl From<std::io::Error> for CoreError { fn from(_: std::io::Error) -> Self { Self::new("io_error", "local I/O operation failed") } }
impl From<rusqlite::Error> for CoreError { fn from(_: rusqlite::Error) -> Self { Self::new("storage_error", "database operation failed") } }
impl From<serde_json::Error> for CoreError { fn from(_: serde_json::Error) -> Self { Self::new("invalid_dto", "malformed or unsupported DTO") } }
