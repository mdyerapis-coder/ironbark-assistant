# Local core

The native daemon owns SQLite, the authenticated local socket, policy and the service
dispatcher. The implemented command surface covers C1 project/status/events/doctor/shutdown,
C5 objects (artifact read, additive import, snapshot backup, offline stopped restore,
conservative retention, capacity) and P2 conversation-tree operations (create, history,
rename, search, export, edit, queue turn, regenerate, archive, delete). Run scheduling and
live provider launch (C4) are not integrated: `run_interrupt` is a deliberate stub and there
is no `run create`/`checkpoint`/`run inspect` command. Native harnesses, provider accounts,
JEV semantic qualification and desktop release are not qualified by this core.

## Build and run

```sh
cargo +1.98.1 build -p ironbark-core -p ironbark-cli
./target/debug/ironbark-core
./target/debug/ironbark-cli doctor --json
./target/debug/ironbark-cli project create example --root /absolute/project/path
./target/debug/ironbark-cli events --after 0
./target/debug/ironbark-cli status
./target/debug/ironbark-cli shutdown
```

The project toolchain pin selects Rust 1.98.1 without requiring a global rustup default.
Startup prints `CORE_READY protocol=1` after opening the store and binding the socket.

## Command surface

Every named subcommand maps to a canonical `CoreCommand` and prints exactly one JSON DTO on
stdout; `artifact read` prints the raw artifact bytes. `--request-id` (global) is the durable
request identity for replay-safety. `backup restore` is the one offline operation: it runs
without a daemon or authentication and installs into the operator-selected XDG namespace.

| Subcommand | Command | Notes |
| --- | --- | --- |
| `status`, `doctor --json`, `shutdown`, `events --after N` | C1 leaves | `events` streams bounded pages until drained |
| `project create NAME --root ABS`, `project list` | C1 leaves | `--root` defaults to the CWD |
| `import preview --project UUID --file ABS [--kind native-transcript --origin STR]` | C5 | preview is a reviewed, additive digest |
| `import apply --project UUID --digest HEX` | C5 | publishes content, then the durable receipt |
| `artifact read --project UUID --id UUID` | C5 | raw bytes to stdout; bounded by the IPC frame |
| `backup create --destination ABS` | C5 | online snapshot via the authenticated daemon |
| `backup restore --source ABS --new-namespace` | C5 | **offline**; stopped, fresh namespace, never overwrites |
| `retention sweep` | C5 | conservative reference-aware CAS reclaim |
| `object-capacity --project UUID` | C5 | published CAS capacity (soft/hard) |
| `conversation create/history/rename/search/export/archive/delete` | P2 | `delete --run UUID` resolves active runs |
| `conversation edit --message UUID PARTS`, `queue-turn`, `regenerate` | P2 | the frame request id is the stable turn identity |
| `command '<CoreCommand JSON>'` | generic | raw wire envelope, machine-parseable |

`conversation queue-turn` and `conversation regenerate` route to the durable leaves using the
frame request id as the turn identity; replay returns the same run, never a duplicate turn.
They do not launch a provider — C4 claims and launches queued work separately. `run_interrupt`
returns `run_dependency_missing` because the core cannot observe native stop and reconcile
terminal state on its own.

## Storage and trust boundary

| Item | Location |
| --- | --- |
| Database | `$XDG_DATA_HOME/ironbark-assistant/core.sqlite3`, falling back to `$HOME/.local/share/ironbark-assistant/core.sqlite3` |
| State | `$XDG_STATE_HOME/ironbark-assistant`, falling back to `$HOME/.local/state/ironbark-assistant` |
| Runtime | `$XDG_RUNTIME_DIR/ironbark-assistant`; without that variable, `<state>/runtime/ironbark-assistant` |
| Runtime files | `core.sock`, `session.nonce`, `core.lock` |
| Content-addressed store | `<data>/objects` (project-owned CAS) |

App directories are 0700; database, nonce, lock and socket are 0600. Directory ancestry is
walked from held descriptors without following symlinks: every component must be
root/current-user-owned, with no group/other write access unless sticky. This permits safe
sticky `/tmp` and ordinary `/run/user` ancestry without chmodding XDG parents. Private files
are opened relative to checked directory descriptors and must be regular, current-user-owned
and single-linked. SQLite opens through a held parent descriptor retained with the checked
database file for its lifetime. A held `core.lock` in the data directory excludes a second
daemon before migration, even when runtime directories differ. The separate runtime lock
protects endpoint ownership.

Both peers check Unix credentials. The first frame must negotiate protocol 1 and present the
runtime nonce; the nonce is never returned by status/events. This boundary permits trusted
same-user clients. It does **not** isolate malicious same-UID applications or root. Untrusted
execution requires the separate worker-containment boundary (see `containment.md`).

Credential-bearing fields, headers and recognisable credential text are redacted before
durable serialization. Raw Cookie/Set-Cookie header values, including folded continuation
lines, are removed in full. Redaction cannot discover arbitrary secrets in unrestricted text;
callers must keep native credential material out of durable messages and action arguments.
Credentials are not public DTO fields or database lease records.

## Request identity and reconnect

A mutating request is accepted under a UUID and command digest before dispatch. Replaying the
identical UUID/command returns the committed reply, not another mutation. Reusing the UUID for
a different command is rejected. Project mutation, its event and final reply commit in one
SQLite transaction. Service commands (C5/P2) are routed by `CoreServices::dispatch` to their
leaf service; the frame request id stays the durable identity, so `complete_request` persists
the redacted reply and marks the request completed under the same id.

Replaying a completed Shutdown request returns its durable reply without stopping the
replacement daemon. Only freshly executed successful Shutdown requests signal termination.

Accepted requests without a committed result are abandoned on restart. They require explicit
reconciliation, not automatic retry. A caller must not interpret disconnection as evidence
that a side effect did not happen.

Event subscriptions are read-only and resume after the last completely decoded sequence.
Persisted sequence—not wall time—determines ordering. The store pages replay with bounded
frames; slow subscribers cannot block unrelated clients or silently drop events. Unstreamable
events are rejected before commit. Malformed/oversized protocol frames and extra command
fields are rejected.

## Offline restore

`backup restore --source ABS --new-namespace` is an owner-selected **offline** operation. It
does not authenticate against a running core and never calls `RuntimePaths::prepare` before
the associated restore API. The source is validated (manifest, object set, hashes, database
integrity and reference closure), installed into a private sibling staging directory, stopped
(nonterminal runs blocked, in-flight requests abandoned, grants revoked, conversation turns
dispatch-claimed), fsynced with a `restore.json` marker, then atomically no-replace installed
into the freshly selected namespace. An existing namespace — even one whose core has stopped —
is refused (`NamespaceExists`). It launches no worker and reacquires no native session.

## Observed verification

On this workstation the integrated Rust suite and the actual prebuilt daemon/CLI were exercised
under disposable XDG directories:

```sh
cargo +1.98.1 test -p ironbark-core -p ironbark-cli
cargo +1.98.1 clippy -p ironbark-core -p ironbark-cli --all-targets -- -D warnings
cargo +1.98.1 run -p ironbark-core --bin export_contracts
bun install --ignore-scripts && bunx tsc --noEmit --strict
bun qa/smoke/core-reconnect.ts
bun qa/smoke/worker-containment.ts
```

- 106 Rust tests passed: durability (16), approval boundary (17), backup/restore (22),
  containment (10), conversation tree (18), dispatch routing (7), network boundaries (16).
- 63 adapter tests passed: provider boundaries (31) and JEV strict decoding (29).
- Clippy (`--all-targets`) and the integrated `tsc --noEmit --strict` type check were clean.
- `core-reconnect`: actual daemon/CLI binaries — status, reconnect, identical replay,
  altered-command rejection, bad-nonce rejection, graceful shutdown, restart, durable SQLite
  and 0700/0600 private permissions.
- `worker-containment`: actual native process/namespace termination evidence for 9 proof
  scenarios (symlink/nonexistent escape, secret home, network allowlist, grandchild cancel,
  runtime-ancestor rename/substitution, live worker budget, ptrace/process_vm, core death,
  private fd3), plus symlink-mount/loader rejection.
- A live import → artifact read → online backup → **offline restore** round-trip returned the
  exact source bytes from a daemon started in the restored namespace, and a second restore
  into an existing namespace was refused with `NamespaceExists`.

This is not a complete application, native harness, provider account, JEV domain or release
qualification. C4 run scheduling/live provider launch, the live provider/JEV/AlphaCode
smokes and the `run create`/`checkpoint` CLI commands remain outstanding.
