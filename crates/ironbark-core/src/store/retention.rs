//! Conservative mark/sweep: every durable artifact and every source revision
//! remains a root. Removing a conversation/revision is a separate authority-
//! checked operation; collection never discards a historical citation to save space.
use std::{collections::{BTreeSet,HashMap},os::fd::AsRawFd, path::PathBuf};
use rusqlite::Connection;
use serde::{Serialize,Deserialize};
use schemars::JsonSchema;
use ts_rs::TS;
use crate::{contracts::Digest,error::{CoreError,Result},paths::private_directory};
use super::{Store,objects::{lease,object_files,safe_ancestors,sync_dir}};

/// Walk the snapshot/transaction, never a second connection to the live database.
/// Only object-bearing digest keys are roots: model/prompt/state digests need not
/// name blobs. Artifact IDs are rooted by the artifacts table, including those
/// nested in checkpoint/receipt/context JSON.
pub(crate) fn referenced_digests(conn:&Connection)->Result<BTreeSet<String>> {
    let mut roots=BTreeSet::new();
    for sql in ["SELECT digest FROM artifacts","SELECT digest FROM attachments","SELECT digest FROM source_revisions"] {
        let mut stmt=conn.prepare(sql)?;
        for row in stmt.query_map([],|r|r.get::<_,String>(0))? {let value=row?;Digest::parse(&value)?;roots.insert(value);}
    }
    let mut artifact_stmt=conn.prepare("SELECT id,digest FROM artifacts")?;
    let artifacts=artifact_stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<HashMap<_,_>,_>>()?;
    let mut tables=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
    let names=tables.query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
    for table in names {
        let quoted=quote(&table);
        let mut info=conn.prepare(&format!("PRAGMA table_info({quoted})"))?;
        let columns=info.query_map([],|r|Ok((r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        for (column,kind) in columns {
            if !kind.eq_ignore_ascii_case("TEXT") {continue;}
            let mut stmt=conn.prepare(&format!("SELECT {} FROM {quoted} WHERE {} IS NOT NULL",quote(&column),quote(&column)))?;
            for row in stmt.query_map([],|r|r.get::<_,String>(0))? {
                if let Ok(value)=serde_json::from_str::<serde_json::Value>(&row?) { collect_json(&value,&mut roots,&artifacts)?; }
            }
        }
    }
    Ok(roots)
}
fn collect_json(value:&serde_json::Value,roots:&mut BTreeSet<String>,artifacts:&HashMap<String,String>)->Result<()> {
    match value {
        serde_json::Value::Object(map)=>for (key,value) in map {
            if matches!(key.as_str(),"revision_digest"|"object_digest"|"content_digest"|"artifact_digest")
                && let Some(digest)=value.as_str() {Digest::parse(digest)?;roots.insert(digest.to_owned());}
            if key=="artifact_id"
                && let Some(id)=value.as_str() { root_artifact(id,roots,artifacts)?; }
            if key=="artifacts"
                && let Some(items)=value.as_array() {
                    for item in items { if let Some(id)=item.as_str() { root_artifact(id,roots,artifacts)?; } }
                }
            collect_json(value,roots,artifacts)?;
        },
        serde_json::Value::Array(items)=>for value in items {collect_json(value,roots,artifacts)?;}, _=>{},
    } Ok(())
}
fn root_artifact(id:&str,roots:&mut BTreeSet<String>,artifacts:&HashMap<String,String>)->Result<()> {
    let digest=artifacts.get(id).ok_or_else(||CoreError::new("IntegrityMismatch","durable JSON references a missing artifact record"))?;
    roots.insert(digest.clone());Ok(())
}
pub(crate) fn quote(name:&str)->String {format!("\"{}\"",name.replace('"',"\"\""))}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema,TS)]
pub struct RetentionReport { #[ts(type="number")] pub removed_objects:u64, #[ts(type="number")] pub removed_staging_files:u64, #[ts(type="number")] pub reclaimed_bytes:u64, #[ts(type="number")] pub retained_objects:u64 }
#[derive(Clone)]
pub struct RetentionService {store:Store,root:PathBuf}
impl RetentionService {
    pub fn new(store:Store,root:PathBuf)->Result<Self> {safe_ancestors(&root)?;private_directory(&root)?;Ok(Self{store,root})}
    pub async fn sweep(&self)->Result<RetentionReport> {
        let root=self.root.clone();
        let guard=tokio::task::spawn_blocking(move ||lease(&root)).await.map_err(|_|CoreError::new("storage_closed","retention lease worker stopped"))??;
        let root=self.root.clone();
        // Immediate transaction excludes reference changes until unlink completes;
        // the filesystem lease excludes publishers and all active backups.
        let (report,_guard)=self.store.with_transaction(move |tx| {
            let roots=referenced_digests(tx)?;let mut report=RetentionReport{removed_objects:0,removed_staging_files:0,reclaimed_bytes:0,retained_objects:0};
            for (digest,bytes) in object_files(&root)? {
                if roots.contains(digest.as_str()) {report.retained_objects+=1;continue;}
                std::fs::remove_file(root.join(digest.as_str()))?;report.removed_objects+=1;report.reclaimed_bytes=report.reclaimed_bytes.saturating_add(bytes);
            }
            let staging=root.join(".staging");
            if staging.exists() {
                safe_ancestors(&staging)?;
                for entry in std::fs::read_dir(&staging)? {
                    let path=entry?.path();let file=super::objects::read_file(&path)?;
                    if unsafe{libc::flock(file.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0 {
                        let error=std::io::Error::last_os_error();
                        if error.raw_os_error()==Some(libc::EWOULDBLOCK) {continue;}
                        return Err(error.into());
                    }
                    let bytes=file.metadata()?.len();std::fs::remove_file(&path)?;
                    report.removed_staging_files+=1;report.reclaimed_bytes=report.reclaimed_bytes.saturating_add(bytes);
                }
                sync_dir(&staging)?;
            }
            sync_dir(&root)?;Ok((report,guard))
        }).await?; Ok(report)
    }
}
