use std::{ffi::CString, fs::{self, File, OpenOptions}, os::{fd::{AsRawFd, FromRawFd}, unix::{fs::{MetadataExt, OpenOptionsExt, PermissionsExt}, ffi::OsStrExt}}, path::{Component, Path, PathBuf}};
use crate::error::{CoreError, Result};

#[derive(Debug, Clone)]
pub struct RuntimePaths { pub data_dir: PathBuf, pub state_dir: PathBuf, pub runtime_dir: PathBuf, pub database: PathBuf, pub socket: PathBuf, pub nonce: PathBuf, pub lock: PathBuf }
pub fn current_uid() -> u32 { unsafe { libc::geteuid() } }

fn safe_directory(file: &File) -> Result<()> {
    let meta = file.metadata()?;
    if !meta.is_dir() || (meta.uid() != 0 && meta.uid() != current_uid())
        || (meta.mode() & 0o022 != 0 && meta.mode() & libc::S_ISVTX == 0) {
        return Err(CoreError::new("unsafe_path", "directory ancestry is replaceable by an untrusted user"));
    }
    Ok(())
}

/// Walk from a held root descriptor without following symlinks. Root/current-user
/// sticky parents are safe only because every child owner is checked as well.
pub(crate) fn directory_handle(path: &Path, create: bool) -> Result<File> {
    if !path.is_absolute() {
        return Err(CoreError::new("unsafe_path", "private paths must be absolute"));
    }
    let mut directory = OpenOptions::new().read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open("/")?;
    safe_directory(&directory)?;
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => CString::new(name.as_bytes())
                .map_err(|_| CoreError::new("unsafe_path", "path component contains NUL"))?,
            _ => return Err(CoreError::new("unsafe_path", "private paths cannot contain parent traversal")),
        };
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let mut fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if !create || error.kind() != std::io::ErrorKind::NotFound { return Err(error.into()); }
            if unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists { return Err(error.into()); }
            }
            fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        }
        let next = unsafe { File::from_raw_fd(fd) };
        safe_directory(&next)?;
        directory = next;
    }
    Ok(directory)
}

pub fn private_directory(path: &Path) -> Result<()> {
    let directory = directory_handle(path, true)?;
    if directory.metadata()?.uid() != current_uid() {
        return Err(CoreError::new("unsafe_path", "private directory must be owned by the current user"));
    }
    directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(crate) fn private_file_at(directory: &File, name: &std::ffi::OsStr) -> Result<File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| CoreError::new("unsafe_path", "private filename contains NUL"))?;
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(),
        libc::O_RDWR | libc::O_CREAT | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
    if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != current_uid() || meta.nlink() != 1 {
        return Err(CoreError::new("unsafe_path", "private file must be a single-linked current-user regular file"));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

pub fn private_file(path: &Path) -> Result<File> {
    let parent = path.parent().ok_or_else(|| CoreError::new("unsafe_path", "private file requires a parent"))?;
    let name = path.file_name().ok_or_else(|| CoreError::new("unsafe_path", "private file requires a filename"))?;
    private_file_at(&directory_handle(parent, false)?, name)
}

fn base(name: &str, fallback: PathBuf) -> Result<PathBuf> {
    let value = std::env::var_os(name).map(PathBuf::from).unwrap_or(fallback);
    if !value.is_absolute() { return Err(CoreError::new("invalid_xdg_path", "XDG paths must be absolute")); }
    Ok(value)
}
impl RuntimePaths {
    pub fn from_env() -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| CoreError::new("missing_home", "HOME is required"))?;
        let data_dir = base("XDG_DATA_HOME", home.join(".local/share"))?.join("ironbark-assistant");
        let state_dir = base("XDG_STATE_HOME", home.join(".local/state"))?.join("ironbark-assistant");
        // Never use a globally shared /tmp fallback for a privileged local endpoint.
        let runtime_dir = base("XDG_RUNTIME_DIR", state_dir.join("runtime"))?.join("ironbark-assistant");
        let socket = runtime_dir.join("core.sock");
        if socket.as_os_str().as_bytes().len() >= 108 { return Err(CoreError::new("socket_path_too_long", "XDG runtime path exceeds Unix socket limit")); }
        Ok(Self { database: data_dir.join("core.sqlite3"), nonce: runtime_dir.join("session.nonce"), lock: runtime_dir.join("core.lock"), data_dir, state_dir, runtime_dir, socket })
    }
    pub fn prepare(&self) -> Result<()> {
        for path in [&self.data_dir, &self.state_dir, &self.runtime_dir] { private_directory(path)?; }
        Ok(())
    }
    pub fn read_nonce(&self) -> Result<String> {
        let directory = directory_handle(&self.runtime_dir, false)?;
        let fd = unsafe { libc::openat(directory.as_raw_fd(), c"session.nonce".as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != current_uid() || meta.mode() & 0o077 != 0 || meta.len() > 256 { return Err(CoreError::new("unsafe_nonce", "nonce file has unsafe ownership, permissions or size")); }
        let mut nonce = String::new();
        std::io::Read::read_to_string(&mut file, &mut nonce)?;
        Ok(nonce.trim().to_owned())
    }
}
