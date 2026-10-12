# Changelog

## Unreleased

- Added the local Rust daemon, private Unix IPC and CLI for projects, status, diagnostics, durable events and shutdown.
- Added transactional request identity: identical replay returns the original reply; changed commands cannot reuse a request ID; interrupted accepted work is not automatically redispatched.
- Added Rust-generated protocol-1 contracts and bounded event replay with precommit credential redaction.
- Enforced unknown-field rejection for payload-free tagged commands and approval decisions; Serde unit variants were insufficient for this boundary.
- Added native exact-action grant and directory-relative file-write primitives, with owner command integration through the service dispatcher.
- Added policy enforcement for exact actions (digest over canonical arguments), single-use durable grants, and the openat2 directory-relative file sink (BENEATH, no magic links, regular single-link files only, nonblocking FIFO refusal, identity re-observation before truncation).
- Added C5 objects: content-addressed artifacts, additive import preview/apply, bounded artifact reads, online snapshot backup, offline stopped restore into a fresh namespace, conservative reference-aware retention and published CAS capacity.
- Added P2 conversation-tree operations: create, history, rename, search, export, message edit, queue turn, regenerate, archive and delete, with the frame request id as the stable turn identity.
- Added CLI named subcommands for the C1/C5/P2 surface plus a generic `command '<CoreCommand JSON>'` dispatch; every subcommand prints exactly one JSON DTO on stdout.
- Added the network lease boundary (deny-by-default, run-pinned, broker-socket re-validated against device/inode, credential-scope header allowlist) and the process namespace boundary: dedicated `namespace_bridge` runner, owned bwrap namespaces, private `/home/worker` + `/run/worker` tmpfs only, non-recursive read-only remounts of `/`, `/home`, `/run`, nondumpable + no_new_privs + TSYNC seccomp denying ptrace/process_vm/pidfd_getfd/x32, pidfd identity tracking and the private fd 3 control channel.
- Corrected worker containment to be runtime-immutable: mounted source roots under `/home/worker` are rejected, and a regression test denies runtime-ancestor rename and substitution of a mounted source via actual process evidence.
- Stopped provider SDK transport retries at the first failed dispatch. Transport failure remains failure, not a user cancellation or another-account fallback.
- Added the TS provider adapter (registry, OMP-SDK, local, custom) with connection pinning, credential-lease resolution over the private fd 3 binary channel, composed cancellation/watchdog signals, output ceilings and exact one-to-one native replay, and the JEV client with strict raw decoding, explicit abstention and abort-on-every-exit.
- Redacted complete raw Cookie/Set-Cookie header values and folded continuation lines before SQLite persistence.
- Prevented completed Shutdown UUID replay from terminating a replacement daemon while preserving the durable reply.
- Acquired a held data-directory authority lock before store recovery, preventing two runtime directories from serving the same store concurrently.
- Rejected unsafe writable or symlinked private-path ancestry and used held directory descriptors for private files and SQLite initialization.
- Verified on this workstation: 106 core Rust tests and 63 adapter tests passing, clippy clean with `-D warnings`, regenerated protocol-1 contracts with a clean strict `tsc` type check, the actual daemon/CLI reconnect smoke, the ten-scenario worker-containment smoke with real process/namespace termination evidence, and a live import, artifact read, online backup and offline restore round-trip with a refused repeat restore (`NamespaceExists`).

The runtime documentation now covers the local core, permissions and policy, worker containment and provider/judgment boundaries. C4 run scheduling and live provider launch, the live provider/JEV/AlphaCode qualification smokes, and the `run create`/`checkpoint` CLI commands remain outstanding; `run_interrupt` is a deliberate stub. Provider/JEV boundary fixtures do not qualify accounts, native harnesses or semantic judgment domains. No desktop package or complete-assistant release is published.
