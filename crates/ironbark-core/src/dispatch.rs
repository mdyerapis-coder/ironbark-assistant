//! Authenticated service dispatch for run-less C5 (objects/imports/backup/retention) and P2
//! (conversation tree) commands.
//!
//! These commands are NOT C1's single-transaction leaf operations: they reach the live Store
//! plus the on-disk content-addressed store, and each leaf commits its own durable mutation and
//! event inside its own transaction. The frame request ID stays the durable identity
//! (`accept_request` de-duplicates and binds it to the command digest); the service does the
//! work; `complete_request` then persists the redacted reply in a fresh transaction and marks
//! the request completed. A crash between accept and complete leaves the request `in_flight` —
//! a reconcile, never an automatic re-dispatch. `backup_restore` is deliberately excluded: it
//! is an offline, owner-selected CLI operation, not a live IPC mutation.
use std::path::{Path, PathBuf};
use uuid::Uuid;

use base64::Engine as _;
use crate::contracts::{ArtifactRead, CoreCommand, CoreReply, ProjectId};
use crate::conversations::ConversationService;
use crate::error::{CoreError, Result};
use crate::paths::RuntimePaths;
use crate::store::{
    backup::BackupService, imports::ImportService, objects::{ObjectLimits, ObjectStore},
    retention::RetentionService, Store,
};

/// A base64 payload plus its JSON envelope must fit the 1 MiB IPC frame with headroom.
const MAX_ARTIFACT_BYTES: u64 = 700_000;

/// Bundles the run-less services behind one request-ID-bound dispatcher. Holds a Store and the
/// durable roots; per-command services are constructed on demand so the daemon keeps one
/// canonical Store and one CAS root for the whole core (ownership is tracked in the artifacts
/// table per project, while CAS files are content-addressed in a single root).
#[derive(Clone)]
pub struct CoreServices {
    pub store: Store,
    paths: RuntimePaths,
}

impl CoreServices {
    pub fn new(store: Store, paths: &RuntimePaths) -> Self { Self { store, paths: paths.clone() } }

    /// True for commands this dispatcher owns (C5 object/import/backup/retention and P2
    /// conversation tree). C1 commands (`Status`/`Events`/`ProjectList`/`ProjectCreate`/
    /// `Doctor`/`Shutdown`) return false and keep the single-transaction path.
    pub fn is_service_command(command: &CoreCommand) -> bool {
        use CoreCommand as C;
        matches!(command,
            C::ArtifactRead { .. } | C::ImportPreview { .. } | C::ImportApply { .. }
            | C::BackupCreate { .. } | C::RetentionSweep { .. } | C::ObjectCapacity { .. }
            | C::ConversationCreate { .. } | C::ConversationHistory { .. } | C::ConversationRename { .. }
            | C::ConversationSearch { .. } | C::ConversationExport { .. } | C::ConversationEdit { .. }
            | C::ConversationArchive { .. } | C::ConversationDelete { .. } | C::ConversationQueueTurn { .. }
            | C::ConversationRegenerate { .. } | C::RunInterrupt { .. })
    }

    fn cas_root(&self) -> PathBuf { self.paths.data_dir.join("objects") }

    fn objects(&self, project: ProjectId) -> Result<ObjectStore> {
        ObjectStore::new(self.store.clone(), project, self.cas_root(), ObjectLimits::default())
    }

    fn imports(&self, project: ProjectId) -> Result<ImportService> {
        ImportService::new(self.objects(project)?)
    }

    /// Route a service command to its leaf service. Produces the reply (or the typed error);
    /// the caller persists it durably under the frame request ID via `dispatch`.
    async fn perform(&self, id: Uuid, command: &CoreCommand) -> Result<CoreReply> {
        use CoreCommand as C;
        match command {
            C::ArtifactRead { project_id, artifact_id } => {
                let mut reader = self.objects(*project_id)?.get(*artifact_id).await?;
                let digest = reader.digest.clone();
                let bytes = reader.bytes;
                if bytes > MAX_ARTIFACT_BYTES {
                    return Err(CoreError::new("artifact_too_large_for_ipc", "artifact exceeds the recoverable IPC frame; use a bulk channel"));
                }
                let mut buf = Vec::with_capacity(bytes as usize);
                std::io::Read::read_to_end(&mut reader, &mut buf)?;
                if buf.len() as u64 != bytes {
                    return Err(CoreError::new("integrity_mismatch", "artifact byte count differs from its manifest"));
                }
                let read = ArtifactRead { digest, bytes, data: base64::engine::general_purpose::STANDARD.encode(&buf) };
                Ok(CoreReply::Ok { value: serde_json::to_value(read)? })
            }
            C::ImportPreview { project_id, source } => {
                let imports = self.imports(*project_id)?;
                Ok(CoreReply::Ok { value: serde_json::to_value(imports.preview(source.clone()).await?)? })
            }
            C::ImportApply { project_id, preview_digest } => {
                let imports = self.imports(*project_id)?;
                Ok(CoreReply::Ok { value: serde_json::to_value(imports.apply(preview_digest.clone()).await?)? })
            }
            C::BackupCreate { destination } => {
                let service = BackupService::new(self.store.clone(), self.cas_root())?;
                Ok(CoreReply::Ok { value: serde_json::to_value(service.create(Path::new(destination)).await?)? })
            }
            C::RetentionSweep {} => {
                let service = RetentionService::new(self.store.clone(), self.cas_root())?;
                Ok(CoreReply::Ok { value: serde_json::to_value(service.sweep().await?)? })
            }
            C::ObjectCapacity { project_id } => {
                Ok(CoreReply::Ok { value: serde_json::to_value(self.objects(*project_id)?.capacity().await?)? })
            }
            C::ConversationCreate { project_id } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.create(*project_id).await?.to_string())? })
            }
            C::ConversationHistory { conversation_id } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.history(*conversation_id).await?)? })
            }
            C::ConversationRename { conversation_id, title } => {
                let service = ConversationService::new(self.store.clone());
                service.rename(*conversation_id, title.clone()).await?;
                Ok(CoreReply::Ok { value: serde_json::json!({}) })
            }
            C::ConversationSearch { project_id, query, include_archived, limit } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.search(*project_id, query.clone(), *include_archived, *limit as usize).await?)? })
            }
            C::ConversationExport { conversation_id, format } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.export(*conversation_id, *format).await?)? })
            }
            C::ConversationEdit { message_id, parts } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.edit(*message_id, parts.clone()).await?.to_string())? })
            }
            // archive/delete are run-less leaf mutations that themselves refuse any
            // nonterminal run (observed_terminal); they do not launch a provider, so they are
            // not C4-scheduler-dependent. They succeed only once C4 has reconciled the runs.
            C::ConversationArchive { conversation_id } => {
                let service = ConversationService::new(self.store.clone());
                service.archive(*conversation_id).await?;
                Ok(CoreReply::Ok { value: serde_json::json!({}) })
            }
            C::ConversationDelete { conversation_id, resolution } => {
                let service = ConversationService::new(self.store.clone());
                service.delete(*conversation_id, resolution.clone()).await?;
                Ok(CoreReply::Ok { value: serde_json::json!({}) })
            }
            // Queueing and regeneration are durable leaf mutations: the frame request ID is
            // the stable turn identity (replay returns the same run, never a duplicate). C4
            // separately claims the queued work and launches the provider; the leaf never
            // invokes a model, so these are not C4-blocked.
            C::ConversationQueueTurn { conversation_id, branch_id, parts, pin, context } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.queue_turn(*conversation_id, *branch_id, id, parts.clone(), pin.clone(), context.clone()).await?)? })
            }
            C::ConversationRegenerate { message_id, pin } => {
                let service = ConversationService::new(self.store.clone());
                Ok(CoreReply::Ok { value: serde_json::to_value(service.regenerate_turn(*message_id, id, pin.clone()).await?)? })
            }
            // Interrupting a run requires the C4 scheduler to observe native stop and commit
            // the reconciled terminal state; the core cannot fabricate that receipt.
            C::RunInterrupt { .. } => {
                Err(CoreError::new("run_dependency_missing", "run_interrupt requires the C4 run scheduler to observe native stop and reconcile terminal state; not yet integrated"))
            }
            _ => Err(CoreError::new("invalid_command", "command is not owned by the service dispatcher")),
        }
    }

    /// Accept the command under the frame request ID (already accepted by the caller), run it,
    /// then durably persist the redacted reply and mark the request completed.
    pub async fn dispatch(&self, id: Uuid, command: &CoreCommand) -> CoreReply {
        let work = self.perform(id, command).await;
        match work {
            Ok(reply) => match self.store.complete_request(id, command, &reply).await {
                Ok(reply) => reply,
                Err(error) => CoreReply::Error { code: error.code, message: error.message, details: serde_json::json!({}) },
            },
            Err(error) => CoreReply::Error { code: error.code, message: error.message, details: serde_json::json!({}) },
        }
    }
}
