# Ironbark Assistant

A standalone workstation assistant: one Rust core owns durable state, policy,
scheduling and processes; Bun adapter workers carry provider and JEV transports.
Adapters never become a second scheduler.

## Components

| Piece | What it is |
| --- | --- |
| `crates/ironbark-core` | Native daemon: SQLite store, authenticated local Unix IPC, policy engine, service dispatcher, worker/network containment boundary. Also builds `namespace_bridge` (the private network bridge staged inside worker namespaces). |
| `crates/ironbark-cli` | Operator CLI. Every subcommand prints exactly one JSON DTO on stdout; `command '<CoreCommand JSON>'` is the generic wire form. `backup restore` runs offline without a daemon. |
| `packages/adapters` | Bun worker: provider registry + OMP/local/custom provider adapters, JEV judgment client. Transport only — no core database access or policy writes. |
| `packages/contracts` | Rust-generated protocol-1 TypeScript contracts (`generated.ts`, `schema.json`). |
| `integrations/alphacode` | Pinned upstream AlphaCode build, patch and notices for the H7 permission-boundary extension. |
| `qa/` | Rust integration tests and Bun smokes (reconnect, worker containment, JEV). |
| `docs/runtime/` | Verified behavior: core, permissions, containment, providers. |
| `docs/design/` | Approved design bundle and implementation plans. |

## Build and run

Toolchain is pinned in `rust-toolchain.toml` (Rust 1.98.1, no global rustup
default required) and `package.json` (Bun 1.4.3).

```sh
cargo build -p ironbark-core -p ironbark-cli
./target/debug/ironbark-core          # prints CORE_READY protocol=1
./target/debug/ironbark-cli doctor --json
./target/debug/ironbark-cli project create example --root /absolute/project/path
./target/debug/ironbark-cli status
./target/debug/ironbark-cli shutdown
```

Check the same way the repository is verified:

```sh
cargo test -p ironbark-core -p ironbark-cli
cargo clippy -p ironbark-core -p ironbark-cli --all-targets -- -D warnings
bun install --ignore-scripts && bunx tsc --noEmit --strict
bun test packages/adapters/test
bun qa/smoke/core-reconnect.ts
bun qa/smoke/worker-containment.ts
```

Worker execution needs `bubblewrap` (namespace isolation) and `bun`
(adapter runtime); the containment smoke exercises real process/namespace
termination evidence.

## Status

The core, policy, C5 objects/backup/restore, P2 conversations, the
network/process containment boundary and the provider/JEV transport contracts
are implemented and verified on this workstation. Run scheduling and live
provider launch (C4), the live provider/JEV/AlphaCode qualification smokes,
and the desktop release are not yet qualified — see `docs/runtime/` for the
exact boundary of what is verified and what is not.

## Packaging

- **Arch (AUR):** `packaging/aur/PKGBUILD` — installs the core, CLI and the
  namespace bridge (`ironbark-assistant` package).
- **RPM:** `packaging/rpm/ironbark-assistant.spec` — built x86_64 `.rpm`
  artifacts are attached to the GitHub releases.

## License

MIT — see [LICENSE](LICENSE).
