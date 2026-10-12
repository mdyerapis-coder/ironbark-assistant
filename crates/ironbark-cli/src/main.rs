//! Same-user Ironbark core client. Structured commands print exactly one JSON DTO on stdout;
//! `artifact read` emits the raw artifact bytes. Nonzero errors carry the real `CoreError`
//! code. `backup restore` is a deliberately offline owner-selected operation: it never
//! authenticates against a running core and never calls `RuntimePaths::prepare` before the
//! associated restore API. All operation arguments are named flags (the canonical CLI shapes).
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use ironbark_core::{
    conversations::{ExportFormat, RunResolution},
    contracts::{
        ArtifactId, BranchId, ContentPart, ConversationId, CoreCommand, CoreReply, Digest, MessageId,
        ProjectId, RunId, RoutePin,
    },
    error::CoreError,
    ipc::CoreClient,
    paths::RuntimePaths,
    store::{backup::BackupService, imports::{ImportFormat, ImportSource}},
};
use std::{io::Write as _, str::FromStr};
use uuid::Uuid;

#[derive(Parser)]
#[command(version, about = "Trusted same-user Ironbark core client")]
struct Args {
    /// Durable request identity for structured commands (replay-safe).
    #[arg(long, global = true)] request_id: Option<Uuid>,
    #[command(subcommand)] command: Command,
}

#[derive(Subcommand)]
enum Command {
    Status,
    Events { #[arg(long, default_value_t = 0)] after: u64 },
    Doctor { #[arg(long)] json: bool },
    Shutdown,
    /// Project lifecycle.
    Project { #[command(subcommand)] command: ProjectCommand },
    /// Additive, reviewed import (document or native transcript).
    Import { #[command(subcommand)] command: ImportCommand },
    /// Read back a durable CAS artifact (raw bytes on stdout).
    Artifact { #[command(subcommand)] command: ArtifactCommand },
    /// Snapshot-derived backup and the offline stopped restore.
    Backup { #[command(subcommand)] command: BackupCommand },
    /// Conservative reference-aware CAS retention sweep.
    Retention { #[command(subcommand)] command: RetentionCommand },
    /// Published CAS capacity report.
    ObjectCapacity { #[arg(long)] project: String },
    /// Conversation tree operations.
    Conversation { #[command(subcommand)] command: ConversationCommand },
    /// Generic core command: the full `CoreCommand` as one JSON argument.
    #[command(name = "command")]
    Raw { #[arg(value_name = "COMMAND_JSON")] json: String },
}

#[derive(Subcommand)]
enum ProjectCommand {
    Create { name: String, #[arg(long)] root: Option<std::path::PathBuf> },
    List,
}

#[derive(Subcommand)]
enum ImportCommand {
    Preview {
        #[arg(long)] project: String,
        #[arg(long)] file: std::path::PathBuf,
        /// document (default) | native-transcript
        #[arg(long, default_value = "document")] kind: String,
        #[arg(long)] origin: Option<String>,
    },
    Apply { #[arg(long)] project: String, #[arg(long)] digest: String },
}

#[derive(Subcommand)]
enum ArtifactCommand {
    Read { #[arg(long)] project: String, #[arg(long)] id: String },
}

#[derive(Subcommand)]
enum BackupCommand {
    Create { #[arg(long)] destination: std::path::PathBuf },
    /// Offline restore into a freshly selected namespace (the current XDG env). Never
    /// authenticates and never overwrites an existing namespace.
    Restore { #[arg(long)] source: std::path::PathBuf, #[arg(long, default_value_t = true)] new_namespace: bool },
}

#[derive(Subcommand)]
enum RetentionCommand {
    Sweep,
}

#[derive(Subcommand)]
enum ConversationCommand {
    Create { #[arg(long)] project: String },
    History { #[arg(long)] conversation: String },
    Rename { #[arg(long)] conversation: String, title: String },
    Search { #[arg(long)] project: String, query: String, #[arg(long, default_value_t = 100)] limit: u32, #[arg(long)] include_archived: bool },
    Export { #[arg(long)] conversation: String, #[arg(long, default_value = "json")] format: String },
    Edit { #[arg(long)] message: String, parts: String },
    QueueTurn { #[arg(long)] conversation: String, #[arg(long)] branch: String, parts: String, #[arg(long)] pin: String },
    Regenerate { #[arg(long)] message: String, #[arg(long)] pin: String },
    Archive { #[arg(long)] conversation: String },
    Delete { #[arg(long)] conversation: String, #[arg(long)] run: Vec<String> },
}

// The core ID newtypes are transparent Uuid wrappers; parse from the CLI string form.
fn project_id(raw: &str) -> Result<ProjectId, String> {
    Ok(ProjectId(Uuid::from_str(raw).map_err(|e| e.to_string())?))
}
fn conversation_id(raw: &str) -> Result<ConversationId, String> {
    Ok(ConversationId(Uuid::from_str(raw).map_err(|e| e.to_string())?))
}
fn branch_id(raw: &str) -> Result<BranchId, String> {
    Ok(BranchId(Uuid::from_str(raw).map_err(|e| e.to_string())?))
}
fn message_id(raw: &str) -> Result<MessageId, String> {
    Ok(MessageId(Uuid::from_str(raw).map_err(|e| e.to_string())?))
}
fn run_id(raw: &str) -> Result<RunId, String> {
    Ok(RunId(Uuid::from_str(raw).map_err(|e| e.to_string())?))
}
fn artifact_id(raw: &str) -> Result<ArtifactId, String> {
    Ok(ArtifactId(Uuid::from_str(raw).map_err(|e| e.to_string())?))
}

async fn run() -> Result<(), String> {
    let args = Args::parse();
    let paths = RuntimePaths::from_env().map_err(display)?;

    // The offline owner-selected restore: no daemon, no auth, no prepare.
    if let Command::Backup { command: BackupCommand::Restore { source, .. } } = &args.command {
        let report = BackupService::restore(source, &paths).await.map_err(|e| e.code.clone())?;
        return print_json(&report);
    }

    let mut client = CoreClient::connect(&paths).await.map_err(display)?;
    let id = args.request_id.unwrap_or_else(Uuid::new_v4);

    // `events` streams bounded pages until the backlog drains; each envelope is one line.
    if let Command::Events { after } = &args.command {
        let mut after = *after;
        loop {
            let reply = client.request(Uuid::new_v4(), CoreCommand::Events { after }).await.map_err(display)?;
            let value = value(reply).map_err(display)?;
            let events: Vec<ironbark_core::contracts::EventEnvelope> = serde_json::from_value(value).map_err(display)?;
            if events.is_empty() { break; }
            for event in events {
                after = event.seq;
                println!("{}", serde_json::to_string(&event).map_err(display)?);
            }
        }
        return Ok(());
    }

    let command = args.command.build().map_err(display)?;
    let reply = client.request(id, command).await.map_err(display)?;
    emit(&paths, reply).map_err(display)
}

impl Command {
    fn build(&self) -> Result<CoreCommand, String> {
        use CoreCommand as C;
        Ok(match self {
            Command::Status => C::Status {},
            Command::Doctor { .. } => C::Doctor {},
            Command::Shutdown => C::Shutdown {},
            Command::Project { command: ProjectCommand::Create { name, root } } => {
                let root = match root.clone() { Some(path) => path, None => std::env::current_dir().map_err(display)? };
                let root = std::fs::canonicalize(&root).map_err(|e| format!("canonicalize {root:?}: {e}"))?;
                C::ProjectCreate { name: name.clone(), root: root.to_string_lossy().into_owned() }
            }
            Command::Project { command: ProjectCommand::List } => C::ProjectList {},
            Command::Import { command: ImportCommand::Preview { project, file, kind, origin } } => {
                let format = match kind.as_str() {
                    "document" => ImportFormat::Document,
                    "native-transcript" => ImportFormat::NativeTranscript { origin: origin.clone().ok_or("import preview --kind native-transcript requires --origin")? },
                    other => return Err(format!("import --kind must be document or native-transcript, got {other:?}")),
                };
                C::ImportPreview { project_id: project_id(project)?, source: ImportSource { path: file.clone(), format } }
            }
            Command::Import { command: ImportCommand::Apply { project, digest } } => {
                C::ImportApply { project_id: project_id(project)?, preview_digest: Digest::parse(digest).map_err(display)? }
            }
            Command::Artifact { command: ArtifactCommand::Read { project, id } } => {
                C::ArtifactRead { project_id: project_id(project)?, artifact_id: artifact_id(id)? }
            }
            Command::Backup { command: BackupCommand::Create { destination } } => {
                C::BackupCreate { destination: destination.to_string_lossy().into_owned() }
            }
            Command::Backup { command: BackupCommand::Restore { .. } } => {
                return Err("backup restore is offline and does not route through the daemon".into())
            }
            Command::Retention { command: RetentionCommand::Sweep } => C::RetentionSweep {},
            Command::ObjectCapacity { project } => C::ObjectCapacity { project_id: project_id(project)? },
            Command::Conversation { command } => command.build()?,
            // Handled directly in run() before build is reached.
            Command::Events { .. } => return Err("events streams directly; use the events subcommand".into()),
            Command::Raw { json } => serde_json::from_str(json).map_err(display)?,
        })
    }
}

impl ConversationCommand {
    fn build(&self) -> Result<CoreCommand, String> {
        use CoreCommand as C;
        let parse_pin = |raw: &str| serde_json::from_str::<RoutePin>(raw).map_err(|e| format!("pin: {e}"));
        let parse_parts = |raw: &str| serde_json::from_str::<Vec<ContentPart>>(raw).map_err(|e| format!("parts: {e}"));
        Ok(match self {
            ConversationCommand::Create { project } => C::ConversationCreate { project_id: project_id(project)? },
            ConversationCommand::History { conversation } => C::ConversationHistory { conversation_id: conversation_id(conversation)? },
            ConversationCommand::Rename { conversation, title } => C::ConversationRename { conversation_id: conversation_id(conversation)?, title: title.clone() },
            ConversationCommand::Search { project, query, limit, include_archived } => C::ConversationSearch { project_id: project_id(project)?, query: query.clone(), limit: *limit, include_archived: *include_archived },
            ConversationCommand::Export { conversation, format } => {
                let format = match format.as_str() {
                    "json" => ExportFormat::Json,
                    "markdown" => ExportFormat::Markdown,
                    other => return Err(format!("export --format must be json or markdown, got {other:?}")),
                };
                C::ConversationExport { conversation_id: conversation_id(conversation)?, format }
            }
            ConversationCommand::Edit { message, parts } => C::ConversationEdit { message_id: message_id(message)?, parts: parse_parts(parts)? },
            ConversationCommand::QueueTurn { conversation, branch, parts, pin } => C::ConversationQueueTurn { conversation_id: conversation_id(conversation)?, branch_id: branch_id(branch)?, parts: parse_parts(parts)?, pin: parse_pin(pin)?, context: None },
            ConversationCommand::Regenerate { message, pin } => C::ConversationRegenerate { message_id: message_id(message)?, pin: parse_pin(pin)? },
            ConversationCommand::Archive { conversation } => C::ConversationArchive { conversation_id: conversation_id(conversation)? },
            ConversationCommand::Delete { conversation, run } => {
                let run_ids = run.iter().map(|r| run_id(r)).collect::<Result<Vec<_>, _>>()?;
                C::ConversationDelete { conversation_id: conversation_id(conversation)?, resolution: RunResolution { run_ids } }
            }
        })
    }
}

// `artifact read` returns base64 in the DTO; the raw bytes are what a caller wants on stdout.
fn emit(_paths: &RuntimePaths, reply: CoreReply) -> Result<(), String> {
    match reply {
        CoreReply::Ok { value } => {
            if let Some(data) = value.get("data").and_then(|d| d.as_str())
                && value.get("digest").is_some() && value.get("bytes").is_some() {
                    let bytes = STANDARD.decode(data).map_err(|e| format!("artifact base64: {e}"))?;
                    let mut out = std::io::stdout().lock();
                    out.write_all(&bytes).map_err(|e| format!("write artifact bytes: {e}"))?;
                    out.flush().map_err(|e| format!("flush artifact bytes: {e}"))?;
                    return Ok(());
                }
            println!("{}", serde_json::to_string_pretty(&value).map_err(display)?);
            Ok(())
        }
        CoreReply::Error { code, message, .. } => Err(format!("{code}: {message}")),
    }
}

fn value(reply: CoreReply) -> Result<serde_json::Value, CoreError> {
    match reply {
        CoreReply::Ok { value } => Ok(value),
        CoreReply::Error { code, message, .. } => Err(CoreError::new(&code, message)),
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(value).map_err(display)?);
    Ok(())
}

fn display<T: std::fmt::Display>(e: T) -> String { e.to_string() }

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
