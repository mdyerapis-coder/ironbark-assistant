use super::PolicyError;
use crate::contracts::{ActionSpec, Digest, ProjectPolicy};
use crate::error::{CoreError, Result};
use serde_json::Value;
use sha2::{Digest as HashDigest, Sha256};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

struct HashWriter<'a>(&'a mut Sha256);
impl Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl ActionSpec {
    pub fn digest(&self) -> Result<Digest> {
        let mut hasher = Sha256::new();
        serde_json::to_writer(HashWriter(&mut hasher), self)
            .map_err(|_| CoreError::new("invalid_action", "Cannot encode action"))?;
        Digest::parse(&format!("{:x}", hasher.finalize()))
    }
}

pub(super) struct ValidatedAction {
    pub consequential: bool,
    pub path: Option<PathBuf>,
}

fn object_with_keys<'a>(value: &'a Value, required: &[&str]) -> std::result::Result<&'a serde_json::Map<String, Value>, PolicyError> {
    let object = value.as_object().ok_or(PolicyError::InvalidAction)?;
    if object.len() != required.len() || required.iter().any(|key| !object.contains_key(*key)) {
        return Err(PolicyError::InvalidAction);
    }
    Ok(object)
}

fn digest_field(value: &Value) -> bool {
    value.as_str().is_some_and(|text| Digest::parse(text).is_ok())
}

pub(super) fn canonical_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() { return None; }
    let mut ancestor = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut root) => {
                for segment in tail.iter().rev() { root.push(segment); }
                return Some(root);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let segment = match ancestor.components().next_back()? {
                    Component::Normal(segment) => segment.to_os_string(),
                    _ => return None,
                };
                tail.push(segment);
                if !ancestor.pop() { return None; }
            }
            Err(_) => return None,
        }
    }
}

pub(super) fn within_roots(path: &Path, roots: &[String]) -> bool {
    roots.iter().any(|root| canonical_path(Path::new(root)).is_some_and(|root| path.starts_with(root)))
}

pub(super) fn validate(action: &ActionSpec, policy: &ProjectPolicy) -> std::result::Result<ValidatedAction, PolicyError> {
    if !policy.allowed_tools.iter().any(|tool| tool == &action.kind) {
        return Err(PolicyError::ForbiddenTool);
    }
    let scope = object_with_keys(&action.scope, &["tools", "paths"])?;
    let tools = scope["tools"].as_array().ok_or(PolicyError::InvalidAction)?;
    let scope_paths = scope["paths"].as_array().ok_or(PolicyError::InvalidAction)?;
    if tools.iter().any(|tool| !tool.as_str().is_some_and(|tool| policy.allowed_tools.iter().any(|allowed| allowed == tool)))
        || !tools.iter().any(|tool| tool.as_str() == Some(action.kind.as_str()))
        || scope_paths.iter().any(|path| !path.as_str().is_some_and(|path| Path::new(path).is_absolute())) {
        return Err(PolicyError::InvalidAction);
    }
    for claimed in scope_paths {
        let resolved = canonical_path(Path::new(claimed.as_str().unwrap()))
            .ok_or(PolicyError::ForbiddenPath)?;
        if !within_roots(&resolved, &policy.read_roots)
            && !within_roots(&resolved, &policy.write_roots) {
            return Err(PolicyError::ForbiddenPath);
        }
    }
    let (required, consequential, path_key): (&[&str], bool, Option<&str>) = match action.kind.as_str() {
        "file_read" => (&["path"], false, Some("path")),
        "file_write" => (&["path", "content_digest"], false, Some("path")),
        "exec" => (&["argv", "cwd"], false, Some("cwd")),
        "integration" => (&["worktree_digest", "base_commit"], true, None),
        "publish" => (&["artifact_digest"], true, None),
        "credential_change" | "connector_change" => (&["reference", "operation"], true, None),
        "disclosure" => (&["artifact_digest", "connection_id"], true, None),
        "browser" | "desktop" => (&["target_id", "observation_digest", "operation", "parameters"], true, None),
        _ => return Err(PolicyError::InvalidAction),
    };
    let args = object_with_keys(&action.canonical_args, required)?;
    for key in ["content_digest", "worktree_digest", "artifact_digest", "observation_digest"] {
        if let Some(value) = args.get(key)
            && !digest_field(value) { return Err(PolicyError::InvalidAction); }
    }
    if action.kind == "exec" {
        let argv = args["argv"].as_array().ok_or(PolicyError::InvalidAction)?;
        if argv.is_empty() || argv.iter().any(|arg| !arg.as_str().is_some_and(|arg| !arg.contains('\0'))) {
            return Err(PolicyError::InvalidAction);
        }
    }
    for key in ["base_commit", "reference", "operation", "connection_id", "target_id"] {
        if let Some(value) = args.get(key)
            && !value.as_str().is_some_and(|text| !text.is_empty() && !text.contains('\0')) {
                return Err(PolicyError::InvalidAction);
            }
    }
    if let Some(connection) = args.get("connection_id").and_then(Value::as_str)
        && !policy.allowed_connection_ids.iter().any(|allowed| allowed == connection) {
            return Err(PolicyError::ForbiddenDestination);
        }
    let path = if let Some(key) = path_key {
        let path = args[key].as_str().ok_or(PolicyError::InvalidAction)?;
        let resolved = canonical_path(Path::new(path)).ok_or(PolicyError::ForbiddenPath)?;
        let roots = if action.kind == "file_write" { &policy.write_roots } else { &policy.read_roots };
        if !within_roots(&resolved, roots) { return Err(PolicyError::ForbiddenPath); }
        if !scope_paths.iter().any(|claimed| claimed.as_str().is_some_and(|claimed| canonical_path(Path::new(claimed)).is_some_and(|claimed| resolved == claimed))) {
            return Err(PolicyError::ForbiddenPath);
        }
        Some(resolved)
    } else { None };
    if let Some(destination) = &action.destination {
        if !super::routes::destination_allowed(destination, &policy.network_destinations)
            || (policy.local_only && !super::routes::is_loopback_destination(destination)) {
            return Err(PolicyError::ForbiddenDestination);
        }
    } else if matches!(action.kind.as_str(), "publish" | "disclosure") {
        return Err(PolicyError::InvalidAction);
    }
    Ok(ValidatedAction { consequential, path })
}
