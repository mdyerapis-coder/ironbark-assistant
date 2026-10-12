use std::{ffi::CString, fs::{File, OpenOptions}, io::{Read, Seek, SeekFrom, Write}, os::{fd::{AsRawFd, FromRawFd}, unix::fs::{MetadataExt, OpenOptionsExt}}, path::{Path, PathBuf}};
use sha2::{Digest as _, Sha256};
use crate::{contracts::*, error::{CoreError, Result}};
use super::{actions, service::{ActionOutcome, ApprovalService, OutcomeEvidence}, PolicyDecision, PolicyEngine};

const MAX_FILE: u64 = 50 * 1024 * 1024;
const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_BENEATH: u64 = 0x08;
#[repr(C)]
struct OpenHow { flags: u64, mode: u64, resolve: u64 }

fn beneath(directory: &File, path: &Path, flags: i32, mode: u32) -> std::io::Result<File> {
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Path contains NUL"))?;
    let how = OpenHow { flags: flags as u64, mode: mode as u64, resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS };
    // openat2 is the authoritative directory-relative boundary; no lexical or
    // unrestricted open fallback when the kernel does not support it.
    let fd = unsafe { libc::syscall(libc::SYS_openat2, directory.as_raw_fd(), path.as_ptr(), &how, std::mem::size_of::<OpenHow>()) };
    if fd < 0 { return Err(std::io::Error::last_os_error()); }
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}

fn hash_bytes(bytes: &[u8]) -> Result<Digest> { Digest::parse(&format!("{:x}", Sha256::digest(bytes))) }
fn regular(file: &File) -> Result<&File> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(CoreError::new("unsupported_target", "Target must be a bounded regular file with one link"));
    }
    Ok(file)
}
fn hash_file(file: &mut File) -> Result<Digest> {
    regular(file)?;
    file.seek(SeekFrom::Start(0))?;
    let before = file.metadata()?;
    if before.len() > MAX_FILE {
        return Err(CoreError::new("unsupported_target", "Target must be a bounded regular file with one link"));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 32 * 1024];
    let mut read = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 { break; }
        read += count as u64;
        if read > MAX_FILE { return Err(CoreError::new("file_too_large", "Target grew past file limit")); }
        hasher.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.mtime() != after.mtime() || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime() || before.ctime_nsec() != after.ctime_nsec() || before.mode() != after.mode() || after.nlink() != 1 {
        return Err(CoreError::new("target_changed", "Target changed during native observation"));
    }
    Digest::parse(&format!("{:x}", hasher.finalize()))
}

struct FileTarget { parent: File, name: PathBuf, file: Option<File>, absolute: PathBuf }
impl FileTarget {
    fn open(policy: &ProjectPolicy, path: &Path, writable: bool) -> Result<Self> {
        let absolute = actions::canonical_path(path).ok_or_else(|| CoreError::new("forbidden_path", "Cannot resolve target"))?;
        let root = policy.write_roots.iter().filter_map(|root| actions::canonical_path(Path::new(root)))
            .filter(|root| absolute.starts_with(root)).max_by_key(|root| root.components().count())
            .ok_or_else(|| CoreError::new("forbidden_path", "Target is outside project write roots"))?;
        let relative = absolute.strip_prefix(&root).map_err(|_| CoreError::new("forbidden_path", "Target escaped root"))?;
        let name = relative.file_name().ok_or_else(|| CoreError::new("invalid_path", "Target must name a file"))?;
        let root_fd = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&root)?;
        let parent_path = relative.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let parent = beneath(&root_fd,parent_path,libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,0)?;
        let name = PathBuf::from(name);
        // Serialize core-owned writers, including creation of previously missing entries.
        // This is advisory; arbitrary external writers are outside this lease.
        let flags = if writable { libc::O_RDWR } else { libc::O_RDONLY };
        // Probe nonblocking so a FIFO cannot occupy the discovery worker before the
        // regular-file check. Writable operations then reopen and lock the same entry.
        let probe = match beneath(&parent, &name, flags | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if writable { fs2::FileExt::try_lock_exclusive(&parent)?; }
                return Ok(Self { parent, name, file: None, absolute });
            }
            Err(error) => return Err(error.into()),
        };
        let probe_meta = probe.metadata()?;
        if !probe_meta.is_file() || probe_meta.nlink() != 1 {
            return Err(CoreError::new("unsupported_target", "Target must be a bounded regular file with one link"));
        }
        let file = if writable {
            let file = match beneath(&parent, &name, flags | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs2::FileExt::try_lock_exclusive(&parent)?;
                    return Ok(Self { parent, name, file: None, absolute });
                }
                Err(error) => return Err(error.into()),
            };
            let file_meta = file.metadata()?;
            if !file_meta.is_file() || file_meta.nlink() != 1
                || file_meta.dev() != probe_meta.dev() || file_meta.ino() != probe_meta.ino() {
                return Err(CoreError::new("target_changed", "Target changed during native discovery"));
            }
            fs2::FileExt::try_lock_exclusive(&file)?;
            file
        } else { probe };
        Ok(Self { parent, name, file: Some(file), absolute })
    }

    fn state(&mut self) -> Result<Digest> {
        use std::os::unix::ffi::OsStrExt;
        let parent = self.parent.metadata()?;
        let mut hasher = Sha256::new();
        hasher.update(b"ironbark-file-state-v1\0");
        hasher.update(parent.dev().to_le_bytes());
        hasher.update(parent.ino().to_le_bytes());
        hasher.update(self.name.as_os_str().as_bytes());
        hasher.update([0]);
        match self.file.as_mut() {
            None => hasher.update(b"missing"),
            Some(file) => {
                let digest = hash_file(file)?;
                let meta = file.metadata()?;
                for field in [meta.dev(),meta.ino(),meta.len(),meta.mode() as u64,meta.uid() as u64,meta.gid() as u64,meta.mtime() as u64,meta.mtime_nsec() as u64,meta.ctime() as u64,meta.ctime_nsec() as u64] {
                    hasher.update(field.to_le_bytes());
                }
                hasher.update(digest.as_str().as_bytes());
            }
        }
        Digest::parse(&format!("{:x}",hasher.finalize()))
    }

    fn write(mut self, bytes: Vec<u8>, expected_state: Digest) -> Result<(Digest,Digest)> {
        let entry = match beneath(&self.parent,&self.name,libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,0) {
            Ok(entry) => Some(entry.metadata()?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let identity_matches = match (&self.file, entry) {
            (None, None) => true,
            (Some(file), Some(entry)) => {
                let owned = file.metadata()?;
                entry.is_file() && entry.dev() == owned.dev() && entry.ino() == owned.ino()
            }
            _ => false,
        };
        if !identity_matches || self.state()? != expected_state {
            return Err(CoreError::new("target_changed", "Target changed before mutation"));
        }
        if self.file.is_none() {
            self.file = Some(beneath(&self.parent,&self.name,libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,0o600)?);
        }
        let file = self.file.as_mut().unwrap();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.parent.sync_all()?;
        let expected = hash_bytes(&bytes)?;
        let actual = hash_file(file)?;
        if actual != expected { return Err(CoreError::new("uncertain_write", "Post-write content differs from requested bytes")); }
        let observed = beneath(&self.parent,&self.name,libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,0)?.metadata()?;
        let owned = file.metadata()?;
        if observed.ino() != owned.ino() || observed.dev() != owned.dev() {
            return Err(CoreError::new("uncertain_write", "Target identity changed after write"));
        }
        Ok((actual,self.state()?))
    }
}

pub struct FileWriteSink;
impl FileWriteSink {
    pub async fn describe(policy: ProjectPolicy, run_id: RunId, path: String, content: Vec<u8>) -> Result<ActionSpec> {
        tokio::task::spawn_blocking(move || {
            if content.len() as u64 > MAX_FILE { return Err(CoreError::new("file_too_large", "Write exceeds file limit")); }
            let mut target = FileTarget::open(&policy,Path::new(&path),false)?;
            let canonical = target.absolute.to_str().ok_or_else(|| CoreError::new("invalid_path", "Target path is not UTF-8"))?;
            let action = ActionSpec { id: ActionId::new(), run_id, project_id: policy.project_id, kind: "file_write".into(), canonical_args: serde_json::json!({"path":canonical,"content_digest":hash_bytes(&content)?}), scope: serde_json::json!({"tools":["file_write"],"paths":[canonical]}), destination: None, policy_version: policy.version, expected_state_digest: target.state()? };
            match PolicyEngine::check(&action,None,None,&policy,0) {
                PolicyDecision::Deny(_) => Err(CoreError::new("policy_denied", "Write is outside policy")),
                _ => Ok(action),
            }
        }).await.map_err(|_| CoreError::new("native_worker_failed", "File observation worker stopped"))?
    }

    pub async fn execute(service: &ApprovalService, action: ActionSpec, grant_id: GrantId, content: Vec<u8>, now: i64) -> Result<Digest> {
        let policy = service.policy(action.project_id).await?;
        if action.kind != "file_write" || content.len() as u64 > MAX_FILE
            || action.canonical_args.get("content_digest").and_then(serde_json::Value::as_str) != Some(hash_bytes(&content)?.as_str())
            || matches!(PolicyEngine::check(&action,None,None,&policy,now),PolicyDecision::Deny(_)) {
            return Err(CoreError::new("invalid_action", "Write bytes or policy do not match the prepared action"));
        }
        let path = action.canonical_args.get("path").and_then(serde_json::Value::as_str).ok_or_else(|| CoreError::new("invalid_action", "Write lacks target path"))?.to_owned();
        let (target,state) = tokio::task::spawn_blocking(move || {
            let mut target = FileTarget::open(&policy,Path::new(&path),true)?;
            let state = target.state()?;
            Ok::<_,CoreError>((target,state))
        }).await.map_err(|_| CoreError::new("native_worker_failed", "File observation worker stopped"))??;
        let consumption = service.consume(action,grant_id,state.clone(),now).await?;
        let expected_state = state.clone();
        let written = tokio::task::spawn_blocking(move || target.write(content,expected_state)).await;
        match written {
            Ok(Ok((content_digest,observed_state_digest))) => {
                service.finish(consumption,OutcomeEvidence { outcome: ActionOutcome::Succeeded, receipt_digest: content_digest.clone(), observed_state_digest },crate::store::events::now_ms()).await?;
                Ok(content_digest)
            }
            _ => {
                // Once consumption is committed, any unverified partial filesystem
                // mutation stays blocked. Never re-arm or silently retry it.
                let _ = service.finish(consumption,OutcomeEvidence { outcome: ActionOutcome::Uncertain, receipt_digest: hash_bytes(b"ironbark-file-outcome-unverified-v1")?, observed_state_digest: state },crate::store::events::now_ms()).await;
                Err(CoreError::new("reconciliation_required", "Write outcome is uncertain; grant remains consumed"))
            }
        }
    }
}
