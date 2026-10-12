use crate::error::{CoreError, Result};
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, File, OpenOptions}, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::{Component, Path, PathBuf}, process::Command};

fn denied(message: &str) -> CoreError { CoreError::new("containment_denied", message) }
/// Exact immutable files only. Directory runtime bundles are deliberately unsupported.
/// Node/Python/cargo packages must enumerate their files, including dynamic dlopen data.
pub struct RuntimeManifest { pub(crate) files: BTreeMap<PathBuf, PathBuf> }
impl RuntimeManifest {
    pub fn empty() -> Self { Self { files: BTreeMap::new() } }
    pub fn merge(&mut self, other: Self) -> Result<()> {
        for (destination,source) in other.files { self.add_file(source,destination)?; }
        Ok(())
    }
    pub fn add_file(&mut self, source: PathBuf, destination: PathBuf) -> Result<()> {
        validate_destination(&destination)?;
        let source = source.canonicalize()?;
        if !source.is_file() { return Err(denied("runtime source must be an exact regular file")); }
        if let Some(old) = self.files.get(&destination) && old != &source { return Err(denied("conflicting runtime destination")); }
        self.files.insert(destination, source); Ok(())
    }
    /// Static readelf inspection only: never executes a candidate binary or ldd.
    /// RUNPATH/RPATH and unresolved libraries fail closed rather than borrowing /usr.
    pub fn discover_elf(executables: &[PathBuf], library_dirs: &[PathBuf]) -> Result<Self> {
        let mut manifest = Self::empty(); let mut queue = executables.to_vec(); let mut visited = BTreeSet::new();
        while let Some(path) = queue.pop() {
            let canonical = path.canonicalize()?; manifest.add_file(canonical.clone(), path.clone())?;
            if !visited.insert(canonical.clone()) { continue; }
            let output = Command::new("/usr/bin/readelf").args(["--wide", "--program-headers", "--dynamic"]).arg(&canonical).env_clear().env("LC_ALL", "C").output()?;
            if !output.status.success() { return Err(denied("runtime is not inspectable ELF; supply its interpreter and exact files explicitly")); }
            let text = std::str::from_utf8(&output.stdout).map_err(|_| denied("invalid readelf output"))?;
            for line in text.lines() {
                if line.contains("(RPATH)") || line.contains("(RUNPATH)") { return Err(denied("ELF search paths require an explicit reviewed runtime manifest")); }
                if let Some(part) = line.split("Requesting program interpreter: ").nth(1) {
                    let interpreter = PathBuf::from(part.trim_end_matches(']').trim()); queue.push(interpreter);
                }
                if line.contains("(NEEDED)") {
                    let name = line.split('[').nth(1).and_then(|s| s.split(']').next()).ok_or_else(|| denied("malformed ELF dependency"))?;
                    if name.contains('/') || name.is_empty() { return Err(denied("non-basename ELF dependency")); }
                    let found = library_dirs.iter().map(|d| d.join(name)).find(|p| p.is_file()).ok_or_else(|| denied("unresolved ELF runtime dependency"))?;
                    queue.push(found);
                }
            }
        }
        Ok(manifest)
    }
}

pub(crate) fn validate_destination(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) || path == Path::new("/") { return Err(denied("mount destination must be an exact absolute path")); }
    if ["/proc", "/sys", "/dev", "/run", "/tmp", "/home/worker"].iter().any(|root| path.starts_with(root)) { return Err(denied("runtime overlaps private or kernel filesystem")); }
    Ok(())
}

/// Resolves the deepest existing ancestor, including symlinks, before checking policy.
/// Mount sources must exist; nonexistent tails are only accepted by path authorization.
pub fn canonical_existing_ancestor(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) { return Err(denied("nonabsolute or traversal path")); }
    let mut ancestor = path; let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(ancestor.file_name().ok_or_else(|| denied("missing path ancestor"))?.to_os_string());
        ancestor = ancestor.parent().ok_or_else(|| denied("missing path ancestor"))?;
    }
    let mut result = ancestor.canonicalize()?; for name in tail.into_iter().rev() { result.push(name); } Ok(result)
}

pub(crate) fn approved_root(path: &Path) -> Result<PathBuf> {
    let root = path.canonicalize()?;
    if !root.is_dir() || root.metadata()?.uid() != unsafe { libc::geteuid() } { return Err(denied("workspace must be an owned directory")); }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if ["/", "/home", "/tmp", "/var", "/mnt", "/run", "/usr", "/etc", "/root", "/proc", "/sys", "/dev"].iter().any(|p| root == Path::new(p)) || home.as_ref() == Some(&root) { return Err(denied("broad host roots cannot be approved workspaces")); }
    if ["/proc", "/sys", "/dev", "/run", "/etc", "/usr", "/root"].iter().any(|p| root.starts_with(p)) { return Err(denied("protected host directory")); }
    Ok(root)
}

pub(crate) fn pin(path: &Path, directory: bool) -> Result<File> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | if directory { libc::O_DIRECTORY } else { 0 }).open(path)?;
    if directory != file.metadata()?.is_dir() { return Err(denied("mount type changed")); }
    Ok(file)
}

pub(crate) fn validate_env(values: &[(String, String)]) -> Result<()> {
    let mut names = BTreeSet::new();
    for (key, value) in values {
        // PATH/HOME/XDG and all language/loader hooks are controlled by the runner.
        if !["LANG", "LC_ALL", "TERM", "NO_COLOR", "TZ"].contains(&key.as_str()) || !names.insert(key) || value.len() > 1024 || value.contains('\0') { return Err(denied("environment variable is not an approved inert runtime setting")); }
    }
    Ok(())
}

pub(crate) fn namespace_id(pid: u32) -> Result<u64> { Ok(fs::metadata(format!("/proc/{pid}/ns/pid"))?.ino()) }

pub(crate) fn reject_submounts(root:&Path)->Result<()> {
    use std::os::unix::ffi::OsStringExt;
    // Recursive bind mounts would otherwise import host mounts nested inside an
    // approved workspace. Kernel mountinfo is authoritative, not path spelling.
    let info=fs::read("/proc/self/mountinfo")?;
    for line in info.split(|b|*b==b'\n') {
        let Some(raw)=line.split(|b|*b==b' ').nth(4) else {continue;};
        let mut decoded=Vec::with_capacity(raw.len());let mut i=0;
        while i<raw.len() {
            if raw[i]==b'\\' && i+3<raw.len() && raw[i+1..i+4].iter().all(|b|(b'0'..=b'7').contains(b)) {
                let byte=(raw[i+1]-b'0') as u16*64+(raw[i+2]-b'0') as u16*8+(raw[i+3]-b'0') as u16;
                decoded.push(u8::try_from(byte).map_err(|_|denied("malformed kernel mount path"))?);i+=4;
            } else {decoded.push(raw[i]);i+=1;}
        }
        let mount=PathBuf::from(std::ffi::OsString::from_vec(decoded));
        if mount!=root && mount.starts_with(root) {return Err(denied("workspace contains a nested host mount"));}
    }
    Ok(())
}
