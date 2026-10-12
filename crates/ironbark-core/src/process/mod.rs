//! Linux containment is native-private. These types are not IPC/frontend DTOs.
mod launcher;
mod supervisor;
pub mod sandbox;
pub use launcher::{harden_managed_process, namespace_runner_entry, NamespaceBootstrap, SiblingCommand};
pub use sandbox::RuntimeManifest;
pub use supervisor::{ProcessIdentity, ProcessLease, ProcessSupervisor, StopReceipt};
use crate::{contracts::RunId, net::NetworkLease};
use std::path::PathBuf;

pub struct LaunchSpec {
    pub run_id: RunId,
    pub executable: PathBuf,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub readonly_mounts: Vec<PathBuf>,
    pub writable_mounts: Vec<PathBuf>,
    pub env_allowlist: Vec<(String, String)>,
    pub network_lease: Option<NetworkLease>,
    pub runtime: RuntimeManifest,
    /// Only a trusted native controller can supply private inherited descriptors.
    /// Authenticated harness/tool isolation remains unqualified until separately proven.
    pub bootstrap: Option<NamespaceBootstrap>,
}
