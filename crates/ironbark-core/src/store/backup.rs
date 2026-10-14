//! Online SQLite snapshot + snapshot-derived immutable object closure. No runtime
//! directory, auth profile, lease, socket or nonce is ever copied. A restore is a
//! data-only, stopped installation into a namespace that did not previously exist.
use std::{collections::BTreeSet, ffi::CString, fs::{self,File}, io::{Read,Write}, os::unix::{ffi::OsStrExt,fs::PermissionsExt}, path::{Path,PathBuf}, time::{Duration,Instant}};
use rusqlite::{Connection,backup::{Backup,StepResult}};
use serde::{Serialize,Deserialize};
use schemars::JsonSchema;
use ts_rs::TS;
use crate::{contracts::Digest,error::{CoreError,Result},paths::{RuntimePaths,private_directory}};
use super::{Store,events::{redact_value,now_ms},objects::{self,Staging,DEFAULT_ITEM_BYTES},retention::{referenced_digests,quote}};

#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema,TS)]
#[serde(deny_unknown_fields)]
pub struct BackupObject {pub digest:Digest,#[ts(type="number")] pub bytes:u64}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema,TS)]
#[serde(deny_unknown_fields)]
pub struct BackupManifest {pub format:u32,#[ts(type="number")] pub created_at:i64,pub database_digest:Digest,#[ts(type="number")] pub database_bytes:u64,pub objects:Vec<BackupObject>}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema,TS)]
pub struct RestoreReport {pub database_digest:Digest,#[ts(type="number")] pub restored_objects:u64,#[ts(type="number")] pub blocked_runs:u64,pub stopped:bool,pub namespace:String}
#[derive(Clone)]
pub struct BackupService {store:Store,root:PathBuf}

/// Linux atomic no-replace installation. An existing file, directory or symlink
/// is never replaced, including one created after the preflight.
pub(crate) fn install_new(source:&Path,target:&Path)->Result<()> {
    let source=CString::new(source.as_os_str().as_bytes()).map_err(|_|CoreError::new("unsafe_path","invalid source path"))?;
    let target=CString::new(target.as_os_str().as_bytes()).map_err(|_|CoreError::new("unsafe_path","invalid target path"))?;
    let status=unsafe {libc::renameat2(libc::AT_FDCWD,source.as_ptr(),libc::AT_FDCWD,target.as_ptr(),libc::RENAME_NOREPLACE)};
    if status==0 {return Ok(());} let error=std::io::Error::last_os_error();
    if error.raw_os_error()==Some(libc::EEXIST) {return Err(CoreError::new("NamespaceExists","destination already exists"));}
    Err(error.into())
}
fn absent(path:&Path)->Result<()> {match fs::symlink_metadata(path) {Ok(_)=>Err(CoreError::new("NamespaceExists","namespace must be explicitly new and absent")),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(e.into())}}
fn stage_for(destination:&Path)->Result<Staging> {
    objects::safe_ancestors(destination)?;absent(destination)?;
    let parent=destination.parent().ok_or_else(||CoreError::new("unsafe_path","destination requires a parent"))?;
    fs::create_dir_all(parent)?;objects::safe_ancestors(parent)?;
    let path=parent.join(format!(".ironbark-stage-{}",uuid::Uuid::new_v4()));
    fs::create_dir(&path)?;private_directory(&path)?;Ok(Staging{path})
}
fn copy_verified(source:&Path,target:&Path,digest:&Digest,bytes:u64)->Result<()> {
    let mut input=objects::read_file(source).map_err(|_|CoreError::new("IntegrityMismatch","referenced backup file is missing or unsafe"))?;
    let mut output=objects::new_file(target)?;let mut copied=0u64;let mut buffer=[0u8;64*1024];
    loop {
        let n=input.read(&mut buffer)?;if n==0 {break;}
        copied=copied.checked_add(n as u64).ok_or_else(||CoreError::new("IntegrityMismatch","source size overflow"))?;
        if copied>bytes {return Err(CoreError::new("IntegrityMismatch","source exceeds declared size"));}
        output.write_all(&buffer[..n])?;
    }
    if copied!=bytes {return Err(CoreError::new("IntegrityMismatch","source differs from declared size"));}
    output.sync_all()?;
    // Hash the actual staged artifact, not just source bytes read before copying.
    objects::verified_file(target,digest,Some(bytes))?;Ok(())
}
fn integrity(conn:&Connection)->Result<()> {
    let unsafe_sql:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE type IN ('trigger','view')",[],|r|r.get(0))?;
    let version:i64=conn.pragma_query_value(None,"user_version",|r|r.get(0))?;
    if unsafe_sql!=0 || version!=1 {return Err(CoreError::new("IntegrityMismatch","unreviewed executable SQL or schema version in snapshot"));}
    let mut stmt=conn.prepare("PRAGMA integrity_check")?;let results=stmt.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
    if results!=["ok"] {return Err(CoreError::new("IntegrityMismatch","SQLite integrity check failed"));}
    let mut stmt=conn.prepare("PRAGMA foreign_key_check")?;if stmt.query([])?.next()?.is_some() {return Err(CoreError::new("IntegrityMismatch","SQLite foreign keys are invalid"));}Ok(())
}

/// The knowledge FTS5 mirror table (owned by `crate::knowledge::chunks`).
const FTS_MIRROR:&str=crate::knowledge::chunks::FTS_TABLE;
/// The mirror plus the shadow tables FTS5 creates for it. All are derived from
/// `chunks` and are reconciled through the virtual table, never scanned/redacted
/// directly (their blob columns are FTS5's own segment index, not user data).
const DERIVED_FTS:&[&str]=&[FTS_MIRROR,"knowledge_fts_data","knowledge_fts_idx","knowledge_fts_content","knowledge_fts_docsize","knowledge_fts_config"];

/// Fail closed for new tables until their data/export ownership is reviewed.
/// SQLite free pages are rebuilt after redaction: UPDATE alone leaks old secrets.
fn sanitize(conn:&mut Connection)->Result<()> {
    integrity(conn)?;
    // The FTS5 knowledge mirror (and the shadow tables FTS5 creates for it) is
    // derived content: `crate::knowledge::chunks::ensure_fts` owns it, no
    // migration creates it, and it indexes the text already held by `chunks`.
    // It is exported (not dropped) because the mirror is also the only place a
    // source's disclosure policy is persisted, so a restored store could not
    // re-derive its filtering from the base tables. Atomicity is inherent: the
    // mirror rides the same single-file online snapshot as `chunks`.
    const ALLOWED:&[&str]=&["projects","policies","provider_references","provider_observations","conversations","branches","messages","branch_messages","attachments","jobs","nodes","runs","conversation_turns","actions","grants","action_outcomes","requests","events","artifacts","checkpoints","sources","source_revisions","chunks","embeddings","memory","judgments","import_previews","import_receipts",crate::knowledge::chunks::FTS_TABLE,"knowledge_fts_data","knowledge_fts_idx","knowledge_fts_content","knowledge_fts_docsize","knowledge_fts_config"];
    let names={let mut stmt=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;let rows=stmt.query_map([],|r|r.get::<_,String>(0))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
    if names.iter().any(|name|!ALLOWED.contains(&name.as_str())) {return Err(CoreError::new("UnknownBackupSchema","new database tables require explicit export review"));}
    let has_mirror=names.iter().any(|name|name==FTS_MIRROR);
    conn.pragma_update(None,"secure_delete","ON")?;
    let tx=conn.transaction()?;
    for table in names {
        let table_name=table;
        // Derived FTS tables are skipped by the generic review: their blob
        // columns are FTS5's own segment index (not unreviewed user data) and
        // their indexed text is reconciled with `chunks` below, so an UPDATE on
        // the virtual table (which FTS5 reflects into its shadow tables in the
        // same transaction) keeps the export exactly consistent with the base
        // rows. Scanning/redacting the shadow tables directly would corrupt the
        // index without changing what a query sees.
        if DERIVED_FTS.contains(&table_name.as_str()) {continue;}
        let table=quote(&table_name);
        let columns={let mut stmt=tx.prepare(&format!("PRAGMA table_info({table})"))?;let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;rows.collect::<std::result::Result<Vec<_>,_>>()?};
        for (column_name,_kind) in columns {
            let column=quote(&column_name);
            let blobs:i64=tx.query_row(&format!("SELECT count(*) FROM {table} WHERE typeof({column})='blob'"),[],|r|r.get(0))?;
            if blobs!=0 && !(table_name=="embeddings" && column_name=="vector") {
                return Err(CoreError::new("UnknownBackupSchema","unreviewed binary data requires an explicit export contract"));
            }
            let mut stmt=tx.prepare(&format!("SELECT rowid,{column} FROM {table} WHERE typeof({column})='text'"))?;
            let mut rows=stmt.query([])?;
            while let Some(row)=rows.next()? {
                let id:i64=row.get(0)?;let text:String=row.get(1)?;
                let parsed=serde_json::from_str::<serde_json::Value>(&text);
                let is_json=parsed.is_ok();
                let mut envelope=serde_json::Map::new();
                envelope.insert(column_name.clone(),parsed.unwrap_or_else(|_|serde_json::Value::String(text.clone())));
                let mut envelope=serde_json::Value::Object(envelope);let before=envelope.clone();redact_value(&mut envelope);
                if envelope==before {continue;} // Preserve exact non-secret text/JSON formatting.
                let value=&envelope[&column_name];
                let cleaned=if is_json {serde_json::to_string(value)?}else{value.as_str().unwrap_or("[REDACTED]").to_owned()};
                if text!=cleaned {tx.execute(&format!("UPDATE {table} SET {column}=?1 WHERE rowid=?2"),rusqlite::params![cleaned,id])?;}
            }
        }
    }
    // Reconcile the derived index with the base rows AFTER redaction so a
    // credential redacted out of `chunks.content` cannot survive as a
    // searchable token in the exported index. Matching by chunk id, and only
    // non-orphan rows, keeps FTS5 and `chunks` exactly consistent.
    if has_mirror {
        tx.execute(&format!("UPDATE {FTS_MIRROR} SET content=(SELECT c.content FROM chunks c WHERE c.id={FTS_MIRROR}.chunk_id) WHERE EXISTS(SELECT 1 FROM chunks c WHERE c.id={FTS_MIRROR}.chunk_id)"),[])?;
    }
    tx.commit()?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; VACUUM;")?;
    integrity(conn)
}

impl BackupService {
    pub fn new(store:Store,root:PathBuf)->Result<Self> {objects::safe_ancestors(&root)?;private_directory(&root)?;Ok(Self{store,root})}
    pub async fn create(&self,destination:&Path)->Result<BackupManifest> {
        let destination=destination.to_path_buf();let root=self.root.clone();
        if destination.starts_with(&root) {return Err(CoreError::new("unsafe_path","backup destination cannot be inside the CAS"));}
        let staging_destination=destination.clone();
        let (stage,guard)=tokio::task::spawn_blocking(move || {
            let guard=objects::lease(&root)?;
            Ok::<_,CoreError>((stage_for(&staging_destination)?,guard))
        }).await.map_err(|_|CoreError::new("storage_closed","backup lease worker stopped"))??;
        let (stage,guard)=self.store.with_connection(move |source| {
            let database=stage.path.join("core.sqlite3");objects::new_file(&database)?;
            let mut snapshot=Connection::open(&database)?;
            {
                let backup=Backup::new(source,&mut snapshot)?;let deadline=Instant::now()+Duration::from_secs(30);
                loop {
                    if Instant::now()>=deadline {return Err(CoreError::new("BackupBusy","online backup exceeded its bounded snapshot budget"));}
                    match backup.step(256)? {StepResult::Done=>break,StepResult::More=>{},StepResult::Busy|StepResult::Locked=>std::thread::sleep(Duration::from_millis(10)),_=>return Err(CoreError::new("BackupBusy","unrecognized SQLite backup progress"))}
                }
            }
            drop(snapshot);
            Ok((stage,guard))
        }).await?;
        let root=self.root.clone();
        tokio::task::spawn_blocking(move || {
            let _guard=guard;let database=stage.path.join("core.sqlite3");
            let mut snapshot=Connection::open(&database)?;
            sanitize(&mut snapshot)?;
            // These references belong to the online snapshot, never a live query.
            let roots=referenced_digests(&snapshot)?;
            let object_dir=stage.path.join("objects");private_directory(&object_dir)?;let mut objects_manifest=Vec::with_capacity(roots.len());
            for value in roots {let digest=Digest::parse(&value)?;
                let bytes=objects::read_file(&root.join(&value)).map_err(|_|CoreError::new("IntegrityMismatch","referenced CAS object is missing or unsafe"))?.metadata()?.len();
                if bytes>DEFAULT_ITEM_BYTES {return Err(CoreError::new("ItemTooLarge","referenced object exceeds the supported item cap"));}
                copy_verified(&root.join(&value),&object_dir.join(&value),&digest,bytes)?;objects_manifest.push(BackupObject{digest,bytes});}
            drop(snapshot);
            let mut file=objects::read_file(&database)?;let (database_digest,database_bytes)=objects::hash_reader(&mut file,u64::MAX)?;
            let manifest=BackupManifest{format:1,created_at:now_ms(),database_digest,database_bytes,objects:objects_manifest};
            let mut file=objects::new_file(&stage.path.join("manifest.json"))?;serde_json::to_writer(&mut file,&manifest)?;file.sync_all()?;
            File::open(&database)?.sync_all()?;objects::sync_dir(&object_dir)?;objects::sync_dir(&stage.path)?;
            install_new(&stage.path,&destination)?;objects::sync_dir(destination.parent().expect("validated parent"))?;Ok(manifest)
        }).await.map_err(|_|CoreError::new("storage_closed","backup copy worker stopped"))?
    }
    pub async fn restore(source:&Path,target:&RuntimePaths)->Result<RestoreReport> {
        let source=source.to_path_buf();let target=target.clone();
        tokio::task::spawn_blocking(move ||restore_stopped(&source,&target)).await.map_err(|_|CoreError::new("storage_closed","restore worker stopped"))?
    }
}

fn restore_stopped(source:&Path,target:&RuntimePaths)->Result<RestoreReport> {
    objects::safe_ancestors(source)?;objects::safe_ancestors(&source.join("objects"))?;
    for path in [&target.data_dir,&target.state_dir,&target.runtime_dir] {objects::safe_ancestors(path)?;absent(path)?;}
    if target.database!=target.data_dir.join("core.sqlite3") || target.socket!=target.runtime_dir.join("core.sock") || target.nonce!=target.runtime_dir.join("session.nonce") || target.lock!=target.runtime_dir.join("core.lock") {
        return Err(CoreError::new("unsafe_path","restore paths must describe one native Ironbark namespace"));
    }
    if target.data_dir==target.state_dir || target.data_dir==target.runtime_dir || target.data_dir.starts_with(&target.state_dir) || target.data_dir.starts_with(&target.runtime_dir) {
        return Err(CoreError::new("unsafe_path","data namespace cannot overlap an ancestor state/runtime namespace"));
    }
    let mut bytes=Vec::new();objects::read_file(&source.join("manifest.json"))?.take(16*1024*1024+1).read_to_end(&mut bytes)?;
    if bytes.len()>16*1024*1024 {return Err(CoreError::new("IntegrityMismatch","backup manifest exceeds limit"));}
    let manifest:BackupManifest=serde_json::from_slice(&bytes).map_err(|_|CoreError::new("IntegrityMismatch","invalid backup manifest"))?;
    if manifest.format!=1 {return Err(CoreError::new("IntegrityMismatch","unsupported backup format"));}
    let mut digests=BTreeSet::new();let mut total=0u64;
    for object in &manifest.objects {if !digests.insert(object.digest.as_str().to_owned()) || object.bytes>DEFAULT_ITEM_BYTES {return Err(CoreError::new("IntegrityMismatch","duplicate or oversized object"));}total=total.checked_add(object.bytes).ok_or_else(||CoreError::new("IntegrityMismatch","object size overflow"))?;}
    let _total=total;
    // All validation and modifications happen in a sibling staging directory.
    let stage=stage_for(&target.data_dir)?;let database=stage.path.join("core.sqlite3");
    copy_verified(&source.join("core.sqlite3"),&database,&manifest.database_digest,manifest.database_bytes)?;
    let mut conn=Connection::open(&database)?;conn.pragma_update(None,"foreign_keys","ON")?;integrity(&conn)?;
    if referenced_digests(&conn)?!=digests {return Err(CoreError::new("IntegrityMismatch","manifest is not exactly the snapshot object closure"));}
    let object_dir=stage.path.join("objects");private_directory(&object_dir)?;
    for object in &manifest.objects {
        let installed=object_dir.join(object.digest.as_str());
        copy_verified(&source.join("objects").join(object.digest.as_str()),&installed,&object.digest,object.bytes)?;
        fs::set_permissions(installed,fs::Permissions::from_mode(0o400))?;
    }
    // Restore also reviews/sanitizes untrusted backup tables before opening the
    // restored core. Existing grants become historical receipts, never authority.
    sanitize(&mut conn)?;
    let tx=conn.transaction()?;
    let blocked_runs=tx.execute("UPDATE runs SET state='blocked' WHERE state NOT IN ('succeeded','failed','cancelled')",[])? as u64;
    tx.execute("UPDATE nodes SET state='blocked' WHERE state NOT IN ('succeeded','failed','cancelled')",[])?;
    tx.execute("UPDATE jobs SET state='blocked' WHERE state NOT IN ('succeeded','failed','cancelled')",[])?;
    tx.execute("UPDATE requests SET state='abandoned',reply=NULL WHERE state='in_flight'",[])?;
    tx.execute("UPDATE conversation_turns SET dispatch_claimed=1",[])?;
    tx.execute("UPDATE messages SET status='interrupted' WHERE status IN ('queued','streaming','awaiting_acceptance') AND id IN (SELECT assistant_message_id FROM conversation_turns WHERE run_id IN (SELECT id FROM runs WHERE state='blocked'))",[])?;
    tx.execute("UPDATE grants SET revoked_at=COALESCE(revoked_at,?1)",[now_ms()])?;
    tx.execute("UPDATE actions SET state='blocked' WHERE state IN ('executing','awaiting_approval','approved')",[])?;
    tx.commit()?;conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;integrity(&conn)?;drop(conn);
    let report=RestoreReport{database_digest:manifest.database_digest,restored_objects:manifest.objects.len() as u64,blocked_runs,stopped:true,namespace:target.data_dir.to_string_lossy().into_owned()};
    let mut marker=objects::new_file(&stage.path.join("restore.json"))?;serde_json::to_writer(&mut marker,&report)?;marker.sync_all()?;
    File::open(&database)?.sync_all()?;objects::sync_dir(&object_dir)?;objects::sync_dir(&stage.path)?;
    // Recheck state/runtime at the point of installation. They are deliberately
    // not created: a normal authenticated startup creates fresh nonce/lock/socket.
    absent(&target.state_dir)?;absent(&target.runtime_dir)?;install_new(&stage.path,&target.data_dir)?;objects::sync_dir(target.data_dir.parent().expect("validated parent"))?;Ok(report)
}
