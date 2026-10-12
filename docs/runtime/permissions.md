# Permissions and policy

The Rust core is the sole policy authority. Adapters and proposals are transports:
they carry a request to the socket, and the core decides. Nothing in the frontend or a
worker can grant, widen or re-decide an action. This document describes the implemented
boundary, not a target.

## Decision surface

`PolicyEngine::check` receives a `TaskAuthority` (task scope, owning run, grant context)
and an action, and returns a `PolicyDecision`. A `TaskScope` carries the project root,
the isolated worktree, and the set of allowed file roots. File roots are checked after
canonicalization: an allowed root must itself be canonical, and a claimed path is
canonicalized component-by-component so a `..` cannot climb out of a declared root.

The core rejects a request it cannot prove safe. Unknown command fields, unknown action
kinds and a missing required argument are all denials, not defaults.

## Exact actions

Each action kind is mapped to a fixed required-argument set, and the action digest is
computed from the canonical arguments, not the caller's raw bytes:

| Action | Required arguments | Notes |
| --- | --- | --- |
| `file_write` | `path`, `content_digest` | content is addressed by digest; the sink re-observes state before truncation |
| `integration` | `worktree_digest`, `base_commit` | bound to an exact worktree and base commit |
| `publish` | `artifact_digest` | publishes an already-stored CAS artifact |
| `disclosure` | `artifact_digest`, `connection_id` | ties disclosure to a registered connection |
| `browser`, `desktop` | `target_id`, `observation_digest`, `operation`, `parameters` | a native observation digest must back the operation |

A caller digest that does not match the recomputed digest over canonical arguments is
denied. `canonical_args` retains the full identity; a truncated or reordered argument set
cannot masquerade as an approved action.

## Single-use grants

A grant is a durable, named permission tied to one action and one run. It is consumed
exactly once: the consumed state is committed, and a revoked grant is refused on every
subsequent check. Replay of an already-consumed grant returns the recorded denial, not a
second effect. Grants are never long-lived standing authority; a fresh grant is required
for a fresh action.

## File-write boundary

File writes are the most sensitive action, so the file sink enforces the boundary at the
descriptor level rather than trusting a path string:

- The target must be a **regular file with exactly one link**. FIFOs, sockets, devices
  and symlinks are refused. The type probe is non-blocking so a FIFO cannot occupy the
  discovery worker before the regular-file check runs.
- Writes go through `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS`, resolved
  relative to a held, already-checked directory descriptor. A path cannot escape its
  rooted directory, and a magic-symlink cannot redirect it.
- Before truncation the sink **re-observes the native state and the directory-entry
  identity**. If the target's identity or observed state changed since the grant was
  checked, the write is refused without truncating (`uncertain_write`).

## What this does not cover

This policy is same-UID trust. It stops the assistant from acting outside an approved
scope, but it does not isolate a malicious same-user application or root. Untrusted
process execution is governed by the separate worker-containment boundary
(`containment.md`). Provider credentials are referenced, never injected through the
policy path — see `providers.md`.

## Observed verification

The 17 approval/file-boundary tests exercise real files: directory-relative writes,
symlink and FIFO refusal, single-use grant consumption, and identity-mismatch refusal
before truncation. These are Rust denials against real filesystem state, not fixture
assertions. They do not qualify a live provider, native harness or JEV domain.
