# Worker containment

The core executes untrusted worker commands inside an owned Bubblewrap namespace
launched by a dedicated `namespace_bridge` binary. The daemon itself never runs
worker code. This document describes the implemented boundary; it is not a claim of
full operating-system isolation.

## Namespace construction

The runner entry (`--ironbark-namespace-runner`) runs **before** the Tokio runtime,
database, IPC or auth services are constructed, then hardens the process:
nondumpable (`PR_SET_DUMPABLE 0`), `PR_SET_NO_NEW_PRIVS`, and a process-wide seccomp
program installed with `SECCOMP_SET_MODE_FILTER` under **TSYNC** so every existing
Tokio/blocking thread is filtered — `prctl` alone would protect only the calling
thread and leave a multithreaded core bypass. The BPF filter is architecture-gated
(x86_64 only, 64-bit syscall class; x32 numbers are rejected) and denies
`ptrace`, `process_vm_readv`, `process_vm_writev` and `pidfd_getfd` with `EPERM`.

Bubblewrap is invoked with a cleared environment:

```
--unshare-all --die-with-parent --new-session --as-pid-1 --cap-drop ALL --clearenv
--proc /proc --dev /dev
--tmpfs /tmp --tmpfs /home --tmpfs /home/worker --tmpfs /run --tmpfs /run/worker
--setenv PATH /usr/bin:/bin --setenv HOME /home/worker
--setenv XDG_RUNTIME_DIR /run/worker --info-fd 6
--remount-ro / --remount-ro /home --remount-ro /run
```

- **Runtime-immutable correction:** `/home` and `/run` are first mounted as empty
  tmpfs, then `/home/worker` and `/run/worker` are created as the **only** private
  writable tmpfs subtrees. `/home` and `/run` are then remounted **read-only,
  non-recursively**, so the host home layout and runtime directory are neither
  visible nor mutable. Source roots that resolve under `/home/worker` are rejected
  before any bind mount, so a worker cannot be handed its own scratch as a "source".
- Only the private child tmpfs and the explicitly approved workspace bind mounts are
  writable. Everything else in the namespace is read-only.
- The child is PID 1 in its own PID namespace; the parent tracks it through
  `pidfd` (with `--info-fd` start-tick identity) so PID reuse cannot turn an
  unrelated host process into an owned child. Double forks and `setsid` cannot
  escape the PID-namespace PID-1 adoption. `--die-with-parent` tears the namespace
  down with the runner.
- `env_clear` plus an explicit small environment means no operator shell state,
  proxies or credential files enter the worker.

## Control channel (fd 3)

The parent opens a `UnixStream` pair and relocates its end into the namespace
controller as an exact, private binary channel: fd 3 is the control stream, fd 4/5
are the null streams, fd 6 is the bubblewrap info fd. The runner verifies it is
PID 1, installs `PR_SET_CHILD_SUBREAPER`, and closes fd 6 and all fds above 6
(`close_range`) after handshaking. A control message that cannot be matched to the
live namespace identity (`start_ticks`) is refused as
`namespace controller identity changed`. Failed handshakes are protected by
timeout, and `bwrap --die-with-parent` kills the namespace if the parent dies.

## Network boundary

Network is denied by default (`--unshare-all` shares no network). A worker only
receives connectivity through a **network lease** whose bootstrap descriptor is
relocated as fd 2 into the namespace bridge. The lease is pinned to a single run id,
has explicit limits (max requests, request/response bytes, lifetime, request
timeout) and is revoked on cancellation. The broker socket is re-validated against
its device/inode before every transport use; a stale or foreign socket is refused.
The credential scope is an **allowlist of header names**
(`authorization`, `x-api-key`, `x-goog-api-key`) resolved from the core's native
secret scope — it is not credential injection, and a `NativeSession` auth reference
is refused as `native_auth_unqualified` until qualified.

## Observed verification

The 10-test containment suite: nine tests produce **live process/namespace evidence**
(not argument inspection), and the tenth is a pre-launch rejection assertion (it
refuses to spawn at all, so it has no termination evidence):

- symlink and nonexistent-path escape attempts are denied
- the operator home is not mounted in the worker namespace
- a worker cannot bypass the network allowlist; a denied egress fails
- cancellation kills the grandchild and the background process
- runtime-ancestor **rename and substitution** of a mounted source are denied
  (the runtime-immutable correction regression)
- the live worker budget is enforced in live namespaces
- `ptrace` and `process_vm_*` are denied by seccomp
- core death terminates the owned namespace
- the private fd 3 control channel is consumed and then absent from the tool
- symlink mounts and unapproved loader options are rejected before any worker starts

The `worker-containment` smoke runs all ten scenarios against the actual prebuilt
core and records the termination evidence. These tests pass on this workstation;
they do not constitute general sandbox-hardness qualification, and live worker
execution against a real provider remains unqualified.
