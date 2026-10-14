//! C5 boundary tests. No fixtures replace the production backup or restore path.
use std::{fs, io::{Cursor, Read}};
use ironbark_core::{contracts::*, paths::RuntimePaths, store::{Store, objects::{ObjectStore, ObjectLimits}, backup::BackupService, retention::RetentionService, imports::{ImportService, ImportSource}}};
use ironbark_core::knowledge::KnowledgeService;
use rusqlite::params;
use tempfile::TempDir;

struct Fixture { temp: TempDir, store: Store, objects: ObjectStore, project: ProjectId, backup: BackupService }
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("core.sqlite3")).await.unwrap();
        let project = ProjectId::new();
        store.with_transaction(move |tx| { tx.execute("INSERT INTO projects VALUES(?1,'backup-test','/not-a-native-profile',0)", [project.to_string()])?; Ok(()) }).await.unwrap();
        let root = temp.path().join("objects");
        let objects = ObjectStore::new(store.clone(), project, root.clone(), ObjectLimits::default()).unwrap();
        let backup = BackupService::new(store.clone(), root).unwrap();
        Self { temp, store, objects, project, backup }
    }
    fn target(&self) -> RuntimePaths {
        let root = self.temp.path().join("new-namespace");
        let data_dir = root.join("data"); let state_dir = root.join("state"); let runtime_dir = root.join("runtime");
        RuntimePaths { database: data_dir.join("core.sqlite3"), socket: runtime_dir.join("core.sock"), nonce: runtime_dir.join("session.nonce"), lock: runtime_dir.join("core.lock"), data_dir, state_dir, runtime_dir }
    }
    async fn references(&self, artifact: ArtifactId) -> RunId {
        let project = self.project; let run = RunId::new(); let job = JobId::new(); let node = NodeId::new();
        self.store.with_transaction(move |tx| {
            let digest: String = tx.query_row("SELECT digest FROM artifacts WHERE id=?1", [artifact.to_string()], |r|r.get(0))?;
            tx.execute("INSERT INTO sources VALUES('old-source',?1,'archive:original')", [project.to_string()])?;
            tx.execute("INSERT INTO source_revisions VALUES('old-revision','old-source',?1,0)", [&digest])?;
            tx.execute("INSERT INTO jobs VALUES(?1,?2,'{}','running')", params![job.to_string(),project.to_string()])?;
            tx.execute("INSERT INTO nodes VALUES(?1,?2,'{}','running')", params![node.to_string(),job.to_string()])?;
            tx.execute("INSERT INTO runs VALUES(?1,?2,'running','{}')", params![run.to_string(),node.to_string()])?;
            tx.execute("INSERT INTO checkpoints VALUES('old-checkpoint',?1,?2,?3)", params![run.to_string(),digest,serde_json::json!({"artifacts":[artifact],"citation":{"revision_digest":digest}}).to_string()])?;
            Ok(())
        }).await.unwrap(); run
    }
}

// Catch a live-query manifest, premature GC, or latest-revision-only backup.
#[tokio::test]
async fn backup_preserves_referenced_objects_consistently() {
    let f = Fixture::new().await;
    let old = f.objects.put(Cursor::new(b"old cited revision".to_vec()), 100).await.unwrap();
    f.references(old).await;
    let new = f.objects.put(Cursor::new(b"new revision".to_vec()), 100).await.unwrap();
    f.store.with_transaction(move |tx| { let digest:String=tx.query_row("SELECT digest FROM artifacts WHERE id=?1",[new.to_string()],|r|r.get(0))?; tx.execute("INSERT INTO source_revisions VALUES('new-revision','old-source',?1,1)",[digest])?; Ok(()) }).await.unwrap();
    RetentionService::new(f.store.clone(), f.temp.path().join("objects")).unwrap().sweep().await.unwrap();
    let destination = f.temp.path().join("backup");
    let manifest = f.backup.create(&destination).await.unwrap();
    assert_eq!(manifest.objects.len(), 2);
    let target=f.target(); BackupService::restore(&destination,&target).await.unwrap();
    let restored=Store::open(&target.database).await.unwrap();
    let objects=ObjectStore::new(restored.clone(),f.project,target.data_dir.join("objects"),ObjectLimits::default()).unwrap();
    let mut reader=objects.get(old).await.unwrap(); let mut bytes=Vec::new(); reader.read_to_end(&mut bytes).unwrap(); assert_eq!(bytes,b"old cited revision");
    let counts=restored.with_connection(|conn| Ok((conn.query_row("SELECT count(*) FROM source_revisions",[],|r|r.get::<_,i64>(0))?, conn.query_row("SELECT count(*) FROM checkpoints",[],|r|r.get::<_,i64>(0))?))).await.unwrap();
    assert_eq!(counts,(2,1));
}

// Catch validation after installation, partial restore, or trusting filename hashes.
#[tokio::test]
async fn missing_or_corrupt_object_aborts_restore() {
    for missing in [false,true] {
        let f=Fixture::new().await; f.objects.put(Cursor::new(b"durable evidence".to_vec()),100).await.unwrap();
        let backup=f.temp.path().join("backup"); let manifest=f.backup.create(&backup).await.unwrap();
        let object=backup.join("objects").join(manifest.objects[0].digest.as_str());
        if missing { fs::remove_file(object).unwrap(); } else { fs::write(object,b"corrupt evidence").unwrap(); }
        let native=f.temp.path().join("native-transcripts"); fs::create_dir(&native).unwrap(); fs::write(native.join("keep"),b"native sentinel").unwrap();
        let target=f.target(); let error=BackupService::restore(&backup,&target).await.unwrap_err();
        assert_eq!(error.code,"IntegrityMismatch"); assert!(!target.data_dir.exists()); assert!(!target.state_dir.exists()); assert!(!target.runtime_dir.exists());
        assert_eq!(fs::read(native.join("keep")).unwrap(),b"native sentinel");
    }
}

#[tokio::test]
async fn restored_runs_do_not_auto_resume() {
    let f=Fixture::new().await; let artifact=f.objects.put(Cursor::new(b"checkpoint".to_vec()),100).await.unwrap(); let run=f.references(artifact).await;
    let backup=f.temp.path().join("backup"); f.backup.create(&backup).await.unwrap(); let target=f.target(); let report=BackupService::restore(&backup,&target).await.unwrap();
    assert_eq!(report.blocked_runs,1); assert!(!target.socket.exists()); assert!(!target.nonce.exists());
    let conn=rusqlite::Connection::open(&target.database).unwrap(); let state:String=conn.query_row("SELECT state FROM runs WHERE id=?1",[run.to_string()],|r|r.get(0)).unwrap(); assert_eq!(state,"blocked");
}

// Catch copying runtime/vault material and leaving secret bytes on SQLite free pages.
#[tokio::test]
async fn export_excludes_secret_leases() {
    let f=Fixture::new().await; let sentinel="C5_SECRET_SENTINEL_39ca2896b9c64b1d";
    fs::write(f.temp.path().join("session.nonce"),sentinel).unwrap();
    f.store.with_transaction(move |tx| { tx.execute("INSERT INTO provider_references VALUES('secret-test',?1)",[serde_json::json!({"api_key":sentinel,"auth_ref":{"type":"secret_service","service":"ironbark","account":"test"}}).to_string()])?; Ok(()) }).await.unwrap();
    let backup=f.temp.path().join("backup"); f.backup.create(&backup).await.unwrap();
    for name in ["core.sqlite3","manifest.json"] { assert!(!fs::read(backup.join(name)).unwrap().windows(sentinel.len()).any(|w|w==sentinel.as_bytes())); }
    assert!(!backup.join("session.nonce").exists());
    let conn=rusqlite::Connection::open(backup.join("core.sqlite3")).unwrap(); let config:String=conn.query_row("SELECT config FROM provider_references WHERE id='secret-test'",[],|r|r.get(0)).unwrap(); assert!(config.contains("secret_service")); assert!(config.contains("REDACTED"));
}

#[tokio::test]
async fn changed_import_preview_requires_review() {
    let f=Fixture::new().await; let path=f.temp.path().join("history.json");
    fs::write(&path,br#"{"kind":"native_transcript","origin":"codex","text":"old transcript"}"#).unwrap();
    let imports=ImportService::new(f.objects.clone()).unwrap();
    let preview=imports.preview(ImportSource::document(path.clone())).await.unwrap();
    fs::write(&path,br#"{"kind":"native_transcript","origin":"codex","text":"changed transcript"}"#).unwrap();
    assert_eq!(imports.apply(preview.digest).await.unwrap_err().code,"ImportChanged");
    let count=f.store.with_connection(|c|Ok(c.query_row("SELECT count(*) FROM conversations",[],|r|r.get::<_,i64>(0))?)).await.unwrap(); assert_eq!(count,0);
}

#[tokio::test]
async fn existing_namespace_is_untouched() {
    let f=Fixture::new().await; let backup=f.temp.path().join("backup"); f.backup.create(&backup).await.unwrap(); let target=f.target();
    fs::create_dir_all(&target.data_dir).unwrap(); fs::write(target.data_dir.join("keep"),b"do not overwrite").unwrap();
    assert_eq!(BackupService::restore(&backup,&target).await.unwrap_err().code,"NamespaceExists");
    assert_eq!(fs::read(target.data_dir.join("keep")).unwrap(),b"do not overwrite");
}

#[tokio::test]
async fn streaming_item_and_store_caps_do_not_publish_partial_rows() {
    let f=Fixture::new().await;
    assert_eq!(f.objects.put(Cursor::new(vec![0;101]),100).await.unwrap_err().code,"ItemTooLarge");
    let objects=ObjectStore::new(f.store.clone(),f.project,f.temp.path().join("objects"),ObjectLimits { item_bytes:100,soft_bytes:3,hard_bytes:6 }).unwrap();
    let id=objects.put(Cursor::new(b"1234".to_vec()),100).await.unwrap(); assert!(objects.capacity().await.unwrap().soft_exceeded);
    assert_eq!(objects.put(Cursor::new(b"5678".to_vec()),100).await.unwrap_err().code,"StoreFull");
    // Same digest does not count twice against physical capacity.
    objects.put(Cursor::new(b"1234".to_vec()),100).await.unwrap();
    let mut data=String::new(); objects.get(id).await.unwrap().read_to_string(&mut data).unwrap(); assert_eq!(data,"1234");
    let another=ProjectId::new(); f.store.with_transaction(move |tx|{tx.execute("INSERT INTO projects VALUES(?1,'other','/',0)",[another.to_string()])?;Ok(())}).await.unwrap();
    let foreign=ObjectStore::new(f.store.clone(),another,f.temp.path().join("objects"),ObjectLimits::default()).unwrap(); assert_eq!(foreign.get(id).await.err().unwrap().code,"ArtifactNotFound");
}

#[tokio::test]
async fn transcript_import_is_additive_archived_and_idempotent() {
    let f=Fixture::new().await; let path=f.temp.path().join("native.json"); let original=br#"{"kind":"native_transcript","origin":"codex/session-42","text":"archived content"}"#;
    fs::write(&path,original).unwrap(); let service=ImportService::new(f.objects.clone()).unwrap();
    let preview=service.preview(ImportSource::document(path.clone())).await.unwrap(); let receipt=service.apply(preview.digest.clone()).await.unwrap(); let again=service.apply(preview.digest).await.unwrap(); assert_eq!(receipt,again);
    assert_eq!(fs::read(path).unwrap(),original);
    assert!(receipt.archived_only); assert!(receipt.artifact_id.is_some());
    let counts=f.store.with_connection(|c|Ok((c.query_row("SELECT count(*) FROM runs",[],|r|r.get::<_,i64>(0))?,c.query_row("SELECT count(*) FROM conversations",[],|r|r.get::<_,i64>(0))?))).await.unwrap(); assert_eq!(counts,(0,0));
}

// Catch collecting blobs reachable only from an old citation or a checkpoint.
#[tokio::test]
async fn retention_marks_old_citations_and_checkpoint_digest_references() {
    let f=Fixture::new().await;
    let citation=f.objects.put(Cursor::new(b"citation-only evidence".to_vec()),100).await.unwrap();
    let checkpoint=f.objects.put(Cursor::new(b"checkpoint-only evidence".to_vec()),100).await.unwrap();
    let orphan=f.objects.put(Cursor::new(b"unreferenced garbage".to_vec()),100).await.unwrap();
    let run=f.references(citation).await;
    let (citation_digest,checkpoint_digest,orphan_digest)=f.store.with_transaction(move |tx|{
        let find=|id:ArtifactId|->ironbark_core::error::Result<String>{Ok(tx.query_row("SELECT digest FROM artifacts WHERE id=?1",[id.to_string()],|r|r.get(0))?)};
        let c=find(citation)?;let p=find(checkpoint)?;let o=find(orphan)?;
        tx.execute("UPDATE checkpoints SET body=?1 WHERE run_id=?2",params![serde_json::json!({"content_digest":p,"citation":{"revision_digest":c}}).to_string(),run.to_string()])?;
        tx.execute("DELETE FROM artifacts",[])?;Ok((c,p,o))
    }).await.unwrap();
    let report=RetentionService::new(f.store.clone(),f.temp.path().join("objects")).unwrap().sweep().await.unwrap();
    assert_eq!(report.removed_objects,1);assert_eq!(report.retained_objects,2);
    assert_eq!(fs::read(f.temp.path().join("objects").join(citation_digest)).unwrap(),b"citation-only evidence");
    assert_eq!(fs::read(f.temp.path().join("objects").join(checkpoint_digest)).unwrap(),b"checkpoint-only evidence");
    assert!(!f.temp.path().join("objects").join(orphan_digest).exists());
    let backup=f.temp.path().join("backup");assert_eq!(f.backup.create(&backup).await.unwrap().objects.len(),2);
}

#[tokio::test]
async fn source_sidecar_change_is_bound_to_import_preview() {
    let f=Fixture::new().await;let sidecar=f.temp.path().join("source.txt");let path=f.temp.path().join("source.json");
    fs::write(&sidecar,b"original source revision").unwrap();fs::write(&path,br#"{"kind":"source","uri":"file:source.txt","path":"source.txt"}"#).unwrap();
    let imports=ImportService::new(f.objects.clone()).unwrap();let preview=imports.preview(ImportSource::document(path.clone())).await.unwrap();
    assert_eq!(preview.files.len(),2);fs::write(&sidecar,b"changed source revision").unwrap();
    assert_eq!(imports.apply(preview.digest).await.unwrap_err().code,"ImportChanged");
    let count=f.store.with_connection(|c|Ok(c.query_row("SELECT count(*) FROM sources",[],|r|r.get::<_,i64>(0))?)).await.unwrap();assert_eq!(count,0);
    let preview=imports.preview(ImportSource::document(path)).await.unwrap();let receipt=imports.apply(preview.digest).await.unwrap();
    let mut bytes=Vec::new();f.objects.get(receipt.artifact_id.unwrap()).await.unwrap().read_to_end(&mut bytes).unwrap();assert_eq!(bytes,b"changed source revision");
}

#[tokio::test]
async fn restore_revokes_unconsumed_authority_and_blocks_all_nonterminal_work() {
    let f=Fixture::new().await;let artifact=f.objects.put(Cursor::new(b"grant evidence".to_vec()),100).await.unwrap();let run=f.references(artifact).await;
    let project=f.project;let action=ActionId::new();let grant=GrantId::new();
    f.store.with_transaction(move |tx|{
        let digest="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        tx.execute("INSERT INTO actions VALUES(?1,?2,?3,?4,1,'approved','{}')",params![action.to_string(),run.to_string(),project.to_string(),digest])?;
        tx.execute("INSERT INTO grants(id,action_id,action_digest,expires_at,body) VALUES(?1,?2,?3,9223372036854775807,'{}')",params![grant.to_string(),action.to_string(),digest])?;
        Ok(())
    }).await.unwrap();
    let backup=f.temp.path().join("backup");f.backup.create(&backup).await.unwrap();let target=f.target();BackupService::restore(&backup,&target).await.unwrap();
    let conn=rusqlite::Connection::open(target.database).unwrap();
    let revoked:i64=conn.query_row("SELECT count(*) FROM grants WHERE revoked_at IS NOT NULL",[],|r|r.get(0)).unwrap();assert_eq!(revoked,1);
    let state:String=conn.query_row("SELECT state FROM actions WHERE id=?1",[action.to_string()],|r|r.get(0)).unwrap();assert_eq!(state,"blocked");
}

#[tokio::test]
async fn import_refuses_symlink_source_without_mutating_native_target() {
    let f=Fixture::new().await;let native=f.temp.path().join("native.json");let content=br#"{"kind":"native_transcript","origin":"codex","text":"original native session"}"#;
    fs::write(&native,content).unwrap();let link=f.temp.path().join("link.json");std::os::unix::fs::symlink(&native,&link).unwrap();
    let imports=ImportService::new(f.objects.clone()).unwrap();assert_eq!(imports.preview(ImportSource::document(link)).await.unwrap_err().code,"unsafe_path");assert_eq!(fs::read(native).unwrap(),content);
}

// The writer uses a real second SQLite connection. Compare references from the
// completed snapshot independently with its manifest, while the live DB changes.
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn online_backup_manifest_matches_snapshot_during_concurrent_writes() {
    use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
    let f=Fixture::new().await;let artifact=f.objects.put(Cursor::new(b"stable online object".to_vec()),100).await.unwrap();
    let digest=f.store.with_connection(move |c|Ok(c.query_row("SELECT digest FROM artifacts WHERE id=?1",[artifact.to_string()],|r|r.get::<_,String>(0))?)).await.unwrap();
    let alternate=f.objects.put(Cursor::new(b"concurrent revision only".to_vec()),100).await.unwrap();
    let alternate_digest=f.store.with_transaction(move |tx|{
        let digest:String=tx.query_row("SELECT digest FROM artifacts WHERE id=?1",[alternate.to_string()],|r|r.get(0))?;
        tx.execute("DELETE FROM artifacts WHERE id=?1",[alternate.to_string()])?;Ok(digest)
    }).await.unwrap();
    let (started_tx,started_rx)=std::sync::mpsc::channel();
    let path=f.temp.path().join("core.sqlite3");let stop=Arc::new(AtomicBool::new(false));let worker_stop=stop.clone();let project=f.project;let writer_digest=alternate_digest;
    let writer=std::thread::spawn(move ||{
        let conn=rusqlite::Connection::open(path).unwrap();conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        let mut iteration=0;
        while !worker_stop.load(Ordering::Acquire) {
            conn.execute("INSERT OR REPLACE INTO artifacts(id,project_id,run_id,digest,path) VALUES('online-row',?1,NULL,?2,?2)",params![project.to_string(),writer_digest]).unwrap();
            conn.execute("DELETE FROM artifacts WHERE id='online-row'",[]).unwrap();iteration+=1;
            if iteration==1 {started_tx.send(()).unwrap();}
        } iteration
    });
    if let Err(error)=started_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        stop.store(true,Ordering::Release);let _=writer.join();panic!("real concurrent writer did not start: {error}");
    }
    let result=f.backup.create(&f.temp.path().join("online-backup")).await;
    stop.store(true,Ordering::Release);let iterations=writer.join().unwrap();assert!(iterations>0);let manifest=result.unwrap();
    let conn=rusqlite::Connection::open(f.temp.path().join("online-backup/core.sqlite3")).unwrap();
    let mut stmt=conn.prepare("SELECT DISTINCT digest FROM artifacts ORDER BY digest").unwrap();let snapshot=stmt.query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<std::result::Result<Vec<_>,_>>().unwrap();
    assert_eq!(snapshot,manifest.objects.iter().map(|o|o.digest.as_str().to_owned()).collect::<Vec<_>>());assert!(snapshot.contains(&digest));
}

#[tokio::test]
async fn export_redacts_unknown_nested_metadata_and_sensitive_columns() {
    let f=Fixture::new().await;let sentinel="C5_METADATA_SECRET_5df462b83f12401d";
    f.store.with_connection(move |conn|{
        conn.execute("ALTER TABLE provider_references ADD COLUMN opaque_secret TEXT",[])?;
        conn.execute("INSERT INTO provider_references(id,config,opaque_secret) VALUES('metadata',?1,?2)",params![serde_json::json!({"unrecognized_metadata":{"nested":[{"lease_material":sentinel}]}}).to_string(),sentinel])?;Ok(())
    }).await.unwrap();
    let backup=f.temp.path().join("backup");f.backup.create(&backup).await.unwrap();
    assert!(!fs::read(backup.join("core.sqlite3")).unwrap().windows(sentinel.len()).any(|w|w==sentinel.as_bytes()));
}

#[tokio::test]
async fn unreviewed_schema_or_sql_trigger_cannot_be_exported() {
    for trigger in [false,true] {
        let f=Fixture::new().await;
        f.store.with_connection(move |conn|{
            if trigger {conn.execute_batch("CREATE TRIGGER replay_after_restore AFTER UPDATE ON runs BEGIN UPDATE runs SET state='running' WHERE id=NEW.id; END;")?;}
            else {conn.execute_batch("CREATE TABLE native_credential_leases(material TEXT); INSERT INTO native_credential_leases VALUES('never-export');")?;}
            Ok(())
        }).await.unwrap();
        let backup=f.temp.path().join("unsafe-backup");let error=f.backup.create(&backup).await.unwrap_err();
        assert!(matches!(error.code.as_str(),"UnknownBackupSchema"|"IntegrityMismatch"));assert!(!backup.exists());
    }
}

#[tokio::test]
async fn actual_native_jsonl_import_is_archived_redacted_not_resumable() {
    let f=Fixture::new().await;let path=f.temp.path().join("native-session.jsonl");let sentinel="C5_NATIVE_LEASE_183c660571f04fcb";
    let bytes=format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"native-42\",\"credential_lease_id\":\"{sentinel}\"}}}}\n{{\"type\":\"message\",\"text\":\"native transcript content\"}}\n");
    fs::write(&path,&bytes).unwrap();let imports=ImportService::new(f.objects.clone()).unwrap();
    let preview=imports.preview(ImportSource::native_transcript(path.clone(),"codex/native-42".into())).await.unwrap();
    let receipt=imports.apply(preview.digest).await.unwrap();assert!(receipt.archived_only);
    let mut archive=String::new();f.objects.get(receipt.artifact_id.unwrap()).await.unwrap().read_to_string(&mut archive).unwrap();
    assert!(!archive.contains(sentinel));assert!(archive.contains("native-42"));assert!(archive.contains("native transcript content"));assert!(archive.contains("REDACTED"));
    assert_eq!(fs::read_to_string(path).unwrap(),bytes);
    let count=f.store.with_connection(|c|Ok(c.query_row("SELECT count(*) FROM runs",[],|r|r.get::<_,i64>(0))?)).await.unwrap();assert_eq!(count,0);
}

#[tokio::test]
async fn replacing_import_with_identical_bytes_requires_identity_review() {
    let f=Fixture::new().await;let path=f.temp.path().join("history.json");let bytes=br#"{"kind":"native_transcript","origin":"codex","text":"same bytes but a different selected file"}"#;
    fs::write(&path,bytes).unwrap();let imports=ImportService::new(f.objects.clone()).unwrap();let preview=imports.preview(ImportSource::document(path.clone())).await.unwrap();
    let replacement=f.temp.path().join("replacement");fs::write(&replacement,bytes).unwrap();fs::rename(replacement,path).unwrap();
    assert_eq!(imports.apply(preview.digest).await.unwrap_err().code,"ImportChanged");
}

#[tokio::test]
async fn default_attachment_cap_cannot_be_raised_by_caller_limit() {
    let f=Fixture::new().await;
    assert_eq!(f.objects.put(std::io::repeat(1).take(50*1024*1024+1),u64::MAX).await.unwrap_err().code,"ItemTooLarge");
    let count=f.store.with_connection(|c|Ok(c.query_row("SELECT count(*) FROM artifacts",[],|r|r.get::<_,i64>(0))?)).await.unwrap();assert_eq!(count,0);
}

#[tokio::test]
async fn connection_import_is_additive_and_preserves_original_identity() {
    let f=Fixture::new().await;let path=f.temp.path().join("connection.json");
    f.store.with_transaction(|tx|{tx.execute("INSERT INTO provider_references VALUES('same','{\"marker\":\"keep-existing-config\"}')",[])?;Ok(())}).await.unwrap();
    fs::write(&path,br#"{"kind":"connection","config":{"id":"same","protocol":"openai_completions","endpoint":"https://example.test/v1","models":[],"allowed_projects":[]}}"#).unwrap();
    let imports=ImportService::new(f.objects.clone()).unwrap();let preview=imports.preview(ImportSource::document(path)).await.unwrap();let receipt=imports.apply(preview.digest).await.unwrap();
    assert_eq!(receipt.origin,"same");assert_ne!(receipt.connection_id.as_deref(),Some("same"));
    let imported=receipt.connection_id.unwrap();let project=f.project;
    f.store.with_connection(move |c|{
        let old:String=c.query_row("SELECT config FROM provider_references WHERE id='same'",[],|r|r.get(0))?;assert_eq!(old,"{\"marker\":\"keep-existing-config\"}");
        let new:String=c.query_row("SELECT config FROM provider_references WHERE id=?1",[imported],|r|r.get(0))?;let new:ConnectionConfig=serde_json::from_str(&new)?;assert_eq!(new.allowed_projects,vec![project]);Ok(())
    }).await.unwrap();
}

#[tokio::test(flavor="multi_thread",worker_threads=4)]
async fn ingestion_concurrency_is_shared_across_project_bound_handles() {
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    struct ObservedReader {active:Arc<AtomicUsize>,peak:Arc<AtomicUsize>,entered:bool,done:bool,byte:u8}
    impl Read for ObservedReader {
        fn read(&mut self,buffer:&mut [u8])->std::io::Result<usize> {
            if self.done {return Ok(0);}
            if !self.entered {self.entered=true;let now=self.active.fetch_add(1,Ordering::SeqCst)+1;self.peak.fetch_max(now,Ordering::SeqCst);}
            std::thread::sleep(std::time::Duration::from_millis(30));buffer[0]=self.byte;self.done=true;Ok(1)
        }
    }
    impl Drop for ObservedReader {fn drop(&mut self){if self.entered{self.active.fetch_sub(1,Ordering::SeqCst);}}}
    let f=Fixture::new().await;let active=Arc::new(AtomicUsize::new(0));let peak=Arc::new(AtomicUsize::new(0));let mut tasks=Vec::new();
    for byte in 0..6 {
        let objects=ObjectStore::new(f.store.clone(),f.project,f.temp.path().join("objects"),ObjectLimits::default()).unwrap();
        let reader=ObservedReader{active:active.clone(),peak:peak.clone(),entered:false,done:false,byte};
        tasks.push(tokio::spawn(async move {objects.put(reader,10).await}));
    }
    for task in tasks {task.await.unwrap().unwrap();}
    assert!(peak.load(Ordering::SeqCst)<=2);assert_eq!(active.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn retention_reclaims_abandoned_staging_but_not_active_ingestion() {
    use std::os::fd::AsRawFd;
    let f=Fixture::new().await;let staging=f.temp.path().join("objects/.staging");let stale=staging.join("abandoned");let active=staging.join("active");
    fs::write(&stale,b"abandoned temporary bytes").unwrap();let file=fs::File::create(&active).unwrap();
    assert_eq!(unsafe{libc::flock(file.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
    let retention=RetentionService::new(f.store.clone(),f.temp.path().join("objects")).unwrap();let report=retention.sweep().await.unwrap();
    assert_eq!(report.removed_staging_files,1);assert!(!stale.exists());assert!(active.exists());
    drop(file);assert_eq!(retention.sweep().await.unwrap().removed_staging_files,1);assert!(!active.exists());
}

#[tokio::test]
async fn restored_partial_turn_retains_content_but_cannot_redispatch() {
    let f=Fixture::new().await;let artifact=f.objects.put(Cursor::new(b"partial checkpoint".to_vec()),100).await.unwrap();let run=f.references(artifact).await;
    let project=f.project;let conversation=ConversationId::new();let branch=BranchId::new();let message=MessageId::new();
    f.store.with_transaction(move |tx|{
        tx.execute("INSERT INTO conversations(id,project_id,created_at) VALUES(?1,?2,0)",params![conversation.to_string(),project.to_string()])?;
        tx.execute("INSERT INTO branches(id,conversation_id) VALUES(?1,?2)",params![branch.to_string(),conversation.to_string()])?;
        tx.execute("INSERT INTO messages(id,branch_id,role,content,created_at,status) VALUES(?1,?2,'assistant','real partial response',0,'streaming')",params![message.to_string(),branch.to_string()])?;
        tx.execute("INSERT INTO conversation_turns(run_id,conversation_id,branch_id,request_id,request_digest,assistant_message_id,snapshot) VALUES(?1,?2,?3,?4,?5,?6,'{}')",params![run.to_string(),conversation.to_string(),branch.to_string(),uuid::Uuid::new_v4().to_string(),"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",message.to_string()])?;
        Ok(())
    }).await.unwrap();
    let backup=f.temp.path().join("backup");f.backup.create(&backup).await.unwrap();let target=f.target();BackupService::restore(&backup,&target).await.unwrap();
    let conn=rusqlite::Connection::open(target.database).unwrap();
    let (content,status):(String,String)=conn.query_row("SELECT content,status FROM messages WHERE id=?1",[message.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();assert_eq!(content,"real partial response");assert_eq!(status,"interrupted");
    let claimed:i64=conn.query_row("SELECT dispatch_claimed FROM conversation_turns WHERE run_id=?1",[run.to_string()],|r|r.get(0)).unwrap();assert_eq!(claimed,1);
}

/// A store that has ingested knowledge must still back up and restore, and the
/// restored store must rank the same cited hit. The FTS5 mirror (and the shadow
/// tables FTS5 creates for it) is derived content: exported, not refused.
#[tokio::test]
async fn backup_after_knowledge_ingest_restores_and_retrieval_still_ranks() {
    let f=Fixture::new().await;
    commit_policy(&f).await;
    let service=KnowledgeService::new(f.store.clone(),f.temp.path().join("objects"));
    let file=f.temp.path().join("notes.txt");
    let bytes=b"the quick brown knowledge fox".to_vec();
    fs::write(&file,&bytes).unwrap();
    // The revision digest is the content digest, so the ingest path itself must
    // publish the same bytes into the CAS: a source revision is always a
    // retention root (see referenced_digests), and nothing here writes it.
    // Ingesting creates the FTS5 mirror lazily; this is the trigger for the gap.
    let revision=service.ingest(f.project,SourceInput::File{path:file.to_string_lossy().into_owned()},DisclosurePolicy::ExplicitPublic).await.unwrap();
    let before=service.retrieve(f.project,"knowledge",10).await.unwrap();
    assert_eq!(before.len(),1,"pre-backup retrieval returns the cited hit");
    let citation=before[0].citation.clone();

    let backup=f.temp.path().join("backup");
    let manifest=f.backup.create(&backup).await.unwrap();
    assert_eq!(manifest.objects.len(),1,"the ingested revision's object closure is exported");
    let target=f.target();
    BackupService::restore(&backup,&target).await.unwrap();

    let restored=Store::open(&target.database).await.unwrap();
    let restored_service=KnowledgeService::new(restored.clone(),target.data_dir.join("objects"));
    let after=restored_service.retrieve(f.project,"knowledge",10).await.unwrap();
    assert_eq!(after.len(),1,"the restored store still ranks the same hit");
    assert_eq!(after[0].citation,citation);
    assert_eq!(after[0].citation.revision_digest.as_str(),revision.digest.as_str());
    assert!(after[0].text.contains("knowledge"));
    // The exported index text is exactly the exported base-row text: no
    // half-restored or divergent mirror.
    let divergent=restored.with_connection(|conn|Ok(conn.query_row("SELECT count(*) FROM knowledge_fts f JOIN chunks c ON c.id=f.chunk_id WHERE f.content != c.content",[],|r|r.get::<_,i64>(0))?)).await.unwrap();
    assert_eq!(divergent,0);
}

/// The derived-FTS exemption is an exact allowlist, not a name prefix, and the
/// fail-closed blob rule still holds for reviewed tables. Both must refuse.
#[tokio::test]
async fn derived_fts_exemption_does_not_widen_the_allowlist() {
    for mutation in ["lookalike_fts","blob_column"] {
        let f=Fixture::new().await;
        f.store.with_connection(move |conn|{
            if mutation=="lookalike_fts" {
                conn.execute_batch("CREATE TABLE knowledge_fts_extra(material TEXT); INSERT INTO knowledge_fts_extra VALUES('never-export');")?;
            } else {
                conn.execute_batch("ALTER TABLE provider_references ADD COLUMN sneak BLOB; INSERT INTO provider_references(id,config,sneak) VALUES('b','{}',x'00ff00ff');")?;
            }
            Ok(())
        }).await.unwrap();
        let backup=f.temp.path().join("unsafe-backup");
        assert_eq!(f.backup.create(&backup).await.unwrap_err().code,"UnknownBackupSchema");
        assert!(!backup.exists());
    }
}

/// The derived index must not retain a token that redaction removed from the
/// base row: the exported FTS mirror is reconciled with `chunks` after redaction.
#[tokio::test]
async fn exported_fts_index_cannot_retain_a_redacted_credential_token() {
    let f=Fixture::new().await;
    let service=KnowledgeService::new(f.store.clone(),f.temp.path().join("objects"));
    let file=f.temp.path().join("creds.txt");
    let bytes=b"release notes api_key: zzsecretzz42 tail".to_vec();
    fs::write(&file,&bytes).unwrap();
    f.objects.put(Cursor::new(bytes),100).await.unwrap();
    service.ingest(f.project,SourceInput::File{path:file.to_string_lossy().into_owned()},DisclosurePolicy::ExplicitPublic).await.unwrap();

    let backup=f.temp.path().join("backup");
    f.backup.create(&backup).await.unwrap();
    let conn=rusqlite::Connection::open(backup.join("core.sqlite3")).unwrap();
    let redacted:i64=conn.query_row("SELECT count(*) FROM chunks WHERE content LIKE '%[REDACTED]%'",[],|r|r.get(0)).unwrap();
    assert_eq!(redacted,1,"the base row was redacted");
    let indexed:i64=conn.query_row("SELECT count(*) FROM knowledge_fts WHERE knowledge_fts MATCH 'zzsecretzz42'",[],|r|r.get(0)).unwrap();
    assert_eq!(indexed,0,"a redacted credential token must not survive as a searchable index term");
    let residual:i64=conn.query_row("SELECT count(*) FROM knowledge_fts WHERE content LIKE '%zzsecretzz42%'",[],|r|r.get(0)).unwrap();
    assert_eq!(residual,0,"the derived index text is reconciled with the redacted base row");
}

/// Commit a minimal policy so retrieval is permitted; no product command
/// commits one yet, so the fixture inserts it exactly as other knowledge tests do.
async fn commit_policy(f:&Fixture) {
    let project=f.project;
    let policy=serde_json::to_string(&ProjectPolicy{version:1,project_id:project,allowed_connection_ids:vec!["local".into()],allowed_tools:vec![],read_roots:vec![],write_roots:vec![],network_destinations:vec![],local_only:false,max_workers:2,max_requests:10,max_cost_microusd:None,spend_policy:SpendPolicy::FreeOnly,allow_unknown_cost:false}).unwrap();
    f.store.with_transaction(move |tx|{tx.execute("INSERT INTO policies(project_id,version,body) VALUES(?1,1,?2)",params![project.to_string(),policy])?;Ok(())}).await.unwrap();
}
