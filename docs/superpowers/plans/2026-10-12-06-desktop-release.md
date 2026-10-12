# Ironbark Desktop and Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the approved Ironbark preview into an actual native workstation application covering every implemented assistant workflow, then verify local delivery end to end.

**Architecture:** Tauri is a presentation/native-dialog client of the durable Rust core. TypeScript view modules project persisted events; execution and permission truth remain in core. The approved original imagery stays ambient, never behind working text.

**Tech Stack:** Tauri 2/WebKitGTK 4.1, TypeScript/Vite without a new frontend framework, DOMPurify/Markdown renderer, xterm 6/fit addon, system fonts, native GTK/KDE-compatible dialogs and Secret Service.

**Spec:** [approved design](../../../DESIGN.md) §§2,10,11; [visual](../../../index.html), [Astra review](../../../ASTRA-REVIEW.md); [master](2026-10-12-ironbark-assistant.md).

## Global Constraints

- The local core is the only scheduler and approval authority.
- Use system fonts and native window decoration.
- No slogans, neon chrome, ornamental badges, blurred glass, or bark textures behind text, code, logs, forms, or approval details.
- Do not regenerate, distort, or remove the artwork's background.
- Credentials stay outside the frontend, prompts, retrieval index, logs, and exports.
- Closing the last window offers “keep tasks running” or “stop tasks”; selecting stop observes actual worker termination.
- Work stops when the workstation is off.

## Review Focus

1. Reconnect/UI reload: real projections recover without duplicate Send or fabricated running state (D1/D2).
2. Malicious Markdown/source/tool text: cannot execute script, IPC or permission decisions (D1/D3).
3. Narrow window/text scaling/keyboard navigation: composer, citations and exact approval details remain accessible (D2/D3).
4. Last-window close/crash/restart: explicit keep/stop, actual descendants and uncertain state reconciled (D4).
5. Local package/restore: complete owned runtime, no secrets/global config damage and truthful capability gates (D5).

---

## Task D1: Native shell, secure core client and real state projection

**Files:** Create `apps/desktop/{package.json,index.html,vite.config.ts}`, `src/{main,core-client,projection,navigation}.ts`; `src-tauri/{Cargo.toml,tauri.conf.json,capabilities/main.json,src/{main,lib,core_client,dialogs}.rs}`; modify root package/workspace scripts. Smoke `qa/smoke/desktop-connect.ts` (native commands plus manual observation checklist).

**Interfaces:** Native `connect_core() -> Result<CoreStatus>`, `core_request(request_id: Uuid, command: CoreCommand) -> Result<CoreReply>`, `subscribe_events(after_seq: u64) -> Result<()>`; TS implements master `CoreClient.request/events`; `Projection.apply(event: EventEnvelope): void`, `rebuild(snapshot: CoreSnapshot): void`. Native session nonce/secret lease never enters JS.

- [ ] At execution, review package requirements against [official Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) and current Arch package metadata. Proposed command: `sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file openssl libappindicator-gtk3 librsvg`; show exact resolved packages before privileged install. Rust is installed per C1, no global toolchain default change. Verify `pkg-config --modversion webkit2gtk-4.1 javascriptcoregtk-4.1` after installation. Do not install now or pipe a moving installer into shell.
- [ ] Implement the native interfaces with a single core connection/snapshot+event cursor, gap detection and snapshot rebuild; request IDs stay stable across reconnect. Tauri capability allowlist exposes typed core operations and native dialogs only, no generic shell/SQL/file read/worker launch. Restrictive CSP, no remote scripts/frames, raw Markdown HTML disabled and DOMPurify on rendered content. External links use an explicit native safe opener, not arbitrary schemes; source citations use exact local revision rendering.
- [ ] Wire `bun run desktop:dev` to start the actual app/core without sample tasks. Empty real state stays empty; doctor/unqualified/error states are genuine. Typecheck/build `bunx tsc --noEmit` and `cargo check -p ironbark-desktop`; expected pass.
- [ ] Launch actual Tauri app. Use computer tooling with live task-scoped human confirmation, then fresh screenshot/accessibility evidence: connect actual daemon, create/select project, reconnect window and render malicious fixture text containing HTML/script/IPC-like instructions. Expected no script/tool/approval effect and correct persisted projection. Use browser helpers only for a supplementary Vite surface, not as a substitute for native proof. Close/release tool control after each verification session.
- [ ] After smoke document native trust/CSP boundaries and installed prerequisites; commit `feat: connect native desktop to durable core`.

## Task D2: Approved conversation-first Ironbark surface

**Files:** Create `src/styles/{tokens,layout,controls}.css`, `src/views/{conversation,sidebar,task-rail}.ts`, `src/components/{composer,markdown,citations,attachments}.ts`; copy approved original artwork to `src/assets/{ironbark-night,ironbark-lock}.jpg` for local approved use; create `src/assets/PROVENANCE.md`; modify navigation/main. Native smoke checklist `qa/smoke/desktop-conversation.md`.

**Interfaces:** `ConversationView.render(state: ConversationProjection): void`; `Composer.submit(input: ComposerDraft): Promise<RunId>` invokes P2 only; `CitationLink.open(citation: Citation): Promise<void>` invokes P3 resolver; `TaskRail.render(run: RunProjection | null): void`.

- [ ] Implement the approved left navigation/project/recent-conversation list, central readable conversation/persistent composer and collapsible right task rail. Model/provider and harness are separate selectors; Chat only advertises no executable tools. Live stream/partial/error/history/branch/edit/regenerate/rename/archive/delete use P2 core commands; repeated Send retains request identity. Unsupported route selections display reason, not disabled-but-unexplained controls. Add real attachment provenance/ingestion progress and exact citation opening.
- [ ] Apply palette exactly: canvas `#0E0C0B`, surface `#161310`, raised `#1E1915`, hover `#29221C`, selected `#40301F`, subtle border `#504236`, control border `#8A7868`, text `#E6D8C8`, secondary `#CBB8A4`, muted `#B5A492`, disabled `#82766A`, accent `#E09040`, on-accent `#1A120C`, focus/warning `#FFC56A`, success `#BCC492`, error `#EE8E79`, info `#A2BFD0`, review `#C5ACCE`. System/Noto Sans, configured mono, body 14 px/20 line height, never below 12 logical px. Australian English with technical identifiers preserved. Artwork only sidebar/quiet new-chat surface, original aspect/background, no regeneration. Record source hashes/provenance; rights remain packaging-gated.
- [ ] Run `bunx tsc --noEmit`, `bun run desktop:build`; expected pass. Do not add source-text/screenshot-string/unit tests for copied palette/layout wiring.
- [ ] Exercise the actual native surface with an approved local live provider: send/stop, attach a real source, open exact citation, edit/regenerate branch, toggle rail and select a separate engine. Capture fresh native screenshots/AX at normal width, 900 px width and 200% text scale; keyboard reaches composer, navigation, citation and controls with visible focus. Expected no fabricated sample content, readable task scope and no artwork beneath work. Compare visually against approved preview/Astra direction, not a new decorative redesign.
- [ ] After smoke document actual UI operations/accessibility findings; commit `feat: deliver approved Ironbark conversation workspace`.

## Task D3: Tasks, knowledge, settings and exact approval surfaces

**Files:** Create `src/views/{tasks,knowledge,settings,approvals,recovery}.ts`, `src/components/{graph,diff,terminal,receipts,source-reader,memory-review}.ts`; native `src-tauri/src/credentials.rs`; modify navigation/core-client; smoke `qa/smoke/desktop-workflows.md`.

**Interfaces:** `ApprovalView.decide(action_id: ActionId, decision: ApprovalDecision): Promise<void>`; `SettingsView.register(input: ConnectionConfig): Promise<string>`; native `open_credential_dialog(reference: AuthRef) -> Result<AuthRef>` handles GTK password entry and native store directly, returning reference only; `TerminalView.attach(run_id: RunId): Promise<void>` accesses owned PTY stream, never launches a shell from JS.

- [ ] Implement real task graph/role/native identities, dependencies, tool receipts, terminal/logs, files/diff, review/verification and integration outcome surfaces using H/C state. Show opaque native children/unknown usage truthfully. Approval details include exact target/destination/action, current diff/snapshot digest, policy scope/expiry; denial is a real core decision. Diff refresh invalidates stale approvals. Recovery view displays observed checkpoint/workspace state and reconciliation choices, not a Retry button for uncertain mutation.
- [ ] Implement knowledge/source revision reader, ingestion/search/deletion, approved-memory review/edit/forget and embedding qualification state. Settings creates/probes/import-previews provider connections, role/skill/MCP activation, route budgets, harness versions/capabilities, WebGPT activation and JEV domain reports. Native Secret Service dialog ensures raw secrets never enter JS; Bitwarden uses explicit references with owner-selected session. Native sign-in/connector/extension changes show exact point-of-risk scope and remain interactive.
- [ ] Run typecheck/build; expected pass. Native smoke performs the real implement/review/verify workflow, denies a write, opens observed diff/receipts, approves a memory candidate and imports a connection preview without modifying original configs. Present a stale action/changed snapshot: UI must require a fresh approval, not reuse the displayed old button. Verify keyboard focus returns correctly after native dialog; untrusted source/tool content cannot invoke IPC or approve anything.
- [ ] After smoke update task/knowledge/settings/consent docs; commit `feat: expose real assistant workflows and approvals`.

## Task D4: Background core, closing windows and recovery UX

**Files:** Create native `src-tauri/src/lifecycle.rs`; core `src/service.rs`; optional user unit `packaging/ironbark-assistant-core.service`; modify core main/desktop commands/projections; native checklist `qa/smoke/desktop-lifecycle.md`.

**Interfaces:** `CoreLifecycle::ensure_running(paths: RuntimePaths) -> Result<CoreStatus>`, `close_last_window(decision: KeepRunning | StopTasks) -> Result<CloseReceipt>`; close receipt records actual run/process outcome, no guessed success. Uses C3/C4.

- [ ] Implement one user-scoped core instance with locked identity/socket. Core is independent of a window and starts only on app use; optional normal systemd user unit, no root service/public listener/login activation. Last-window close asks keep/stop when active; KeepRunning leaves the actual core working, StopTasks waits for observed native/process termination or reports Blocked failure. Core shutdown/crash behavior preserves checkpoints and kills owned worker groups safely; launching another window reconnects rather than starting duplicate core/worker.
- [ ] Run focused C4 recovery tests and native build. Native smoke starts a real harmless coding task, chooses KeepRunning, reopens and observes continued actual run; repeats StopTasks and checks descendants exit. Kill/restart core around temporary mutation and use recovery UI to reconcile without another write. Expected preserved history, no duplicate dispatch, accurate blocked/cancelled labels and no lost partial artifact.
- [ ] After smoke document optional service/no-login activation/close semantics; commit `feat: preserve task lifecycle independently of windows`.

## Task D5: Local packaging and complete end-to-end acceptance

**Files:** Create `packaging/{local-install.sh,manifest.json,licenses/}`, `qa/qualification/release.ts`, `docs/runtime/{installation,acceptance,capabilities}.md`; modify build scripts/Cargo/Bun locks/CHANGELOG only for verified final behavior.

**Interfaces:** `releaseCheck(manifest: ReleaseManifest): Promise<ReleaseReport>`; manifest binds app/core/adapter versions, source/build/schema hashes, native tool paths, qualified capability reports, licenses and artwork disposition. `ReleaseReport = Passed | Blocked {gates}`; missing/failed required gate cannot be ignored with a green overall result.

- [ ] Package a user-local Linux x86_64 installation with explicit core/adapter/runtime paths, owned native desktop entry and uninstall manifest. Preserve user/native harness configs/sessions and shared JEV service; no moving latest download, silent updater or mock runtime. Keep all needed notices; audit transitive licenses and Firefox-extension distribution. Get explicit artwork rights/attribution disposition before a distributable package; local preview approval is not redistribution permission. Do not publish/upload anything as part of local delivery.
- [ ] Run final `cargo test --workspace`, `bun test packages/adapters/test`, `bunx tsc --noEmit`, `bun run desktop:build`; expected all pass with no skipped required qualification. Run `bun qa/qualification/release.ts --manifest packaging/manifest.json`; it checks actual versioned receipts/coverage and negative gates, not mere file presence. Deliberately remove one required gate in a throwaway manifest: expected Blocked; restore legitimate manifest afterward. This tests checker integrity, not a permanent test of copies.
- [ ] Install/launch the actual local package under fresh XDG test directories and exercise the full design acceptance: permitted provider/source citation → qualified local JEV advisory → approved OMP implementation → different native harness read-only review → independent actual verification → exact approved integration. Repeat permitted workflow via Codex WebGPT, exercise AlphaCode genuine coding and bounded browser/AT-SPI, then deny consequential action and prove absent effect. Every other included engine receives its real session/approval/abort qualification, not a terminal screenshot. Keep native-provider safety/consent human-owned.
- [ ] Exercise real backup/restore into fresh state, source update/deletion/citation behavior, interrupted task reconciliation, revoked/stale approvals, credential redaction, close/stop descendants and actual UI accessibility. Evidence is fresh native screenshot/AX plus core/native tool/process/filesystem receipts. Report unsupported optional capabilities individually, but required stages W/J/AlphaCode cannot be silently omitted from overall full delivery.
- [ ] After complete smoke and owner review, update runtime/docs/changelog with only exercised results, remove throwaway fixture state/processes, commit `feat: deliver verified workstation assistant`. Use finishing/verification skills at actual execution completion. No release claim or implementation begins during this planning handoff.
