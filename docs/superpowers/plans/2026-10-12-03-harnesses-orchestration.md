# Native Harnesses and Orchestration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run actual OMP, Codex, OpenCode, Claude Code, Hermes and AlphaCode workers under one graph/policy authority, including bounded browser/desktop actions.

**Architecture:** Versioned native adapters preserve sessions and real tools, with core-owned worktrees, approvals, process/network leases and results. Roles/skills/tools are scoped configuration, not authority. AlphaCode receives a small pinned enforcement extension rather than replacing the scheduler.

**Tech Stack:** Bun/TypeScript; ACP SDK 1.8.0; OMP SDK 18.8.9; native Codex app-server; Claude Agent SDK 0.3.296/installed CLI 2.1.296; Rust core/Bubblewrap; pinned AlphaCode 1.0.79.

**Spec:** [design](../../../DESIGN.md) §§5,9; [AlphaCode](../../../ALPHACODE-ASSESSMENT.md); [master contracts](2026-10-12-ironbark-assistant.md).

## Global Constraints

- The local core is the only scheduler and approval authority.
- A terminal launcher is not a complete orchestration adapter.
- One writer owns each worktree; shared edits go through one integration owner.
- No claim that every provider can run through every harness.
- A role definition is not an approval grant.
- If a harness cannot honor the policy under the selected environment, refuse unattended dispatch and explain why.

## Review Focus

1. Hidden subagents/providers or missing native callbacks: bound parent and mark opacity; never fake compliance (H1–H5/H7).
2. Native resume/cwd mismatch: reject, do not borrow another project's session (H2/H3/H5/H7).
3. Graph cycle, competing writers, failed prerequisite or exhausted budget: reject/queue/block deterministically (H6).
4. Skill/hook/MCP injection or direct-tool execution bypass: activation cannot grant authority (H6/H7).
5. Hostile page/desktop content, stale UI target or Wayland-unsupported input: fresh target-bound consent and honest failure (H8).

---

## Task H1: Shared native boundary, worktree ownership and real OMP

**Files:** Create adapters `src/harness/{contract,acp,events,omp}.ts`; core `src/workspaces/{mod,worktrees,diffs}.rs`, `src/execution/{mod,adapters,results}.rs`; tests `packages/adapters/test/native-boundaries.test.ts`, core `tests/worktree_ownership.rs`; smoke `qa/smoke/harness.ts`.

**Interfaces:** Consumes C1–C4/P1. Produces master `HarnessAdapter`; `HarnessRegistry::describe(kind: HarnessKind) -> Result<HarnessDescriptor>`, `ExecutionService::open(input: OpenSessionInput) -> Result<SessionRef>`, `submit(session: SessionRef, input: TurnInput) -> Result<RunId>`; `WorkspaceService::acquire(project: ProjectId, run: RunId, mode: WorkspaceMode) -> Result<WorktreeLease>`, `integrate(lease: WorktreeLease, grant: ApprovalGrant) -> Result<IntegrationReceipt>`; `WorkspaceMode = ReadOnly | Writer`, `WorktreeLease {project_id,run_id,path,base_commit,mode}`.

- [ ] Add tests `denied_native_write_has_no_receipt_or_effect`, `unknown_permission_request_fails_closed`, `one_writer_per_worktree`, `switch_harness_creates_new_session_with_provenance`, `native_done_requires_acceptance`. Assert `assert_eq!(second_writer_error, CoreError::WorkspaceBusy);` and `expect(deniedFileExists).toBe(false); expect(newSession.native_id).not.toBe(oldSession.native_id);` with old-session provenance retained.
- [ ] Run `bun test packages/adapters/test/native-boundaries.test.ts` and `cargo test -p ironbark-core --test worktree_ownership`; expected initially FAIL.
- [ ] Implement shared ACP framing/version checks, native permission correlation and event translation; use OMP `omp acp` and native RPC/SDK extensions only for features ACP cannot represent. Reuse genuine OMP agent/session/tool APIs rather than reconstructing its loop. Bind explicit route, project rules, scoped tools and core-issued grants; unknown requests deny. Stage approved extension/skill dependencies into the immutable worker bundle. Native child events carry true identity or explicit opaque status; opaque children cannot evade parent budgets/route policy.
- [ ] Implement actual Git worktrees from the project-selected base, read-only reviewer leases and one integration owner. Detect dirty/non-Git projects and require explicit workspace choice instead of modifying the original or creating an implicit destructive baseline. Integration checks current base/diff digest, approval and review/verification receipts before applying. Never automatic checkout/reset/merge of unrelated changes.
- [ ] Run tests and `bun qa/smoke/harness.ts --harness omp --connection <approved-id>`. Actual OMP implements a temporary tiny CLI change, emits native tool failure, is denied an out-of-scope write, pauses/resumes its actual session, then cancels a child command. Expected real diff/check receipts, denied sentinel absent, native IDs stable on same-engine resume, no remaining children. This is qualification, not fixture replay.
- [ ] After smoke document OMP supported transport/version/limitations; commit `feat: run genuine OMP workers in owned worktrees`.

## Task H2: Genuine Codex app-server adapter

**Files:** Create `src/harness/codex.ts`, `src/native/codex-rpc.ts`; generated `src/native/codex-schema/`; tests `test/codex-session.test.ts`; modify registry; extend harness smoke.

**Interfaces:** Implements `HarnessAdapter`; `CodexNativeClient.startThread(input: OpenSessionInput) -> Promise<SessionRef>`, `resumeThread(reference: SessionRef) -> Promise<SessionRef>`, `startTurn(input: TurnInput) -> AsyncIterable<WorkerEvent>`. Consumes shared permission/event/process contract.

- [ ] Add tests `resume_rejects_project_or_route_mismatch`, `unknown_server_request_is_denied`, `late_tool_failure_is_not_success`, `native_cancel_waits_for_observed_stop`. Assert `expect(resumed.native_id).toBe(original.native_id); await expect(mismatchedResume).rejects.toThrow("SessionIdentityMismatch");` and the actual native thread/turn/tool IDs survive translation.
- [ ] Run `bun test packages/adapters/test/codex-session.test.ts`; expected initially FAIL.
- [ ] Generate bindings with installed `codex app-server generate-ts`/`generate-json-schema` into the declared schema directory, record CLI 0.162.1 schema digest; use stdio `codex app-server --stdio --strict-config`. No websocket/public control or analytics-default-enabled. Dedicated managed home/config, native `on-request` approvals and actual supported sandbox mode compatible with C3. Verify effective provider/model; disable nested provider fallback. Reuse local WebGPT adapter's native request/deny patterns but not its hardcoded route or native-tool-disable list for ordinary Codex coding.
- [ ] Run tests; `bun qa/smoke/harness.ts --harness codex --connection <qualified-native-id>` exercises actual coding, denied write, streaming, tool error, resume and cancel. Expected native authority retained, approved diff only, correct stop and no global config change.
- [ ] After smoke document schema pin/route compatibility; commit `feat: add native Codex session adapter`.

## Task H3: OpenCode ACP/native adapter

**Files:** Create `src/harness/opencode.ts`, `src/native/opencode.ts`; test `test/opencode-session.test.ts`; modify registry/smoke.

**Interfaces:** Implements `HarnessAdapter`; native extension `OpenCodeClient.exportBoundedHistory(session: SessionRef) -> Promise<ContextPackage>`.

- [ ] Add `permission_denial_precedes_tool_execution`, `provider_unavailable_does_not_switch_route`, `resume_cannot_cross_project`, `unexpected_listener_is_unqualified`; assert `expect(deniedFileExists).toBe(false); expect(resolvedRoute).toEqual(selectedRoute); expect(listenerCapability.state).toBe("unqualified");`.
- [ ] Run `bun test packages/adapters/test/opencode-session.test.ts`; expected initially FAIL.
- [ ] Use installed `opencode acp --pure --cwd <worktree>` with private managed configuration and permission bridge; native server only for missing ACP features. ACP help permits a loopback port; qualify any created listener inside the worker namespace, no mDNS, wildcard hostname or permissive CORS. Do not use a globally shared `opencode serve` as an unauthenticated core backend. Explicitly activated plugins are separate scoped runtime bundles, not `--pure` silently discarded features.
- [ ] Run tests then real `bun qa/smoke/harness.ts --harness opencode --connection <qualified-id>` covering text/tools/denial/session continuation/cancel. Expected genuine OpenCode events/artifacts and contained listener/process shutdown.
- [ ] After smoke update compatibility docs/changelog; commit `feat: add bounded OpenCode native adapter`.

## Task H4: Claude Code native host-permission adapter

**Files:** Create `src/harness/claude.ts`, `src/native/claude-host.ts`; test `test/claude-permissions.test.ts`; modify adapter package lock/registry/smoke.

**Interfaces:** Implements `HarnessAdapter`; `ClaudePermissionHost.canUseTool(nativeRequest: NativePermissionRequest) -> Promise<NativePermissionResult>` invokes core C2, never model/self approval.

- [ ] Add `host_denial_is_not_native_allow`, `settings_validation_failure_blocks_start`, `session_snapshot_change_is_visible_on_resume`, `fallback_model_is_never_added`. Assert `expect(deniedFileExists).toBe(false); expect(invalidSettingsDispatchCount).toBe(0);` and invalid settings cannot silently widen defaults.
- [ ] Run `bun test packages/adapters/test/claude-permissions.test.ts`; expected initially FAIL.
- [ ] Pin Claude Agent SDK 0.3.296, explicitly select installed CLI 2.1.296, use genuine streamed input/output and host `canUseTool` callbacks. Installed CLI exposes `--permission-mode manual`, `--permission-prompts host`, `--restricted`, `--strict-mcp-config` and explicit tool lists; configure equivalent SDK options, inspect its exact option typings before calls. Never `auto`, `bypassPermissions`, dangerous-skip, `--fallback-model`, cloud/remote-control or unmanaged MCP. `--restricted` removes command tools unless explicitly named: add only authorised native tools, not fake replacements. Validate managed settings before launch because print mode may silently ignore invalid settings. Preserve native system-prompt snapshot/resume semantics; context changes require visible new session or supported update.
- [ ] Run tests then `bun qa/smoke/harness.ts --harness claude --connection <qualified-native-id>`. Expected actual edit, host-denied effect absent, genuine partial stream/history, resumed identity and stopped descendants. Native subscription/auth use remains owner-approved.
- [ ] After smoke update Claude activation/settings docs; commit `feat: preserve Claude native approvals and sessions`.

## Task H5: Hermes ACP adapter

**Files:** Create `src/harness/hermes.ts`, `src/native/hermes-profile.ts`; test `test/hermes-permissions.test.ts`; modify registry/smoke.

**Interfaces:** Implements `HarnessAdapter`; `HermesProfile.prepare(input: OpenSessionInput) -> Promise<ManagedProfileReceipt>` with isolated config/state/auth refs.

- [ ] Add `headless_mode_cannot_bypass_approval`, `profile_model_mismatch_blocks_resume`, `hook_request_without_activation_is_denied`, `cancel_requires_child_stop`. Assert `expect(deniedFileExists).toBe(false); expect(mismatchedResumeDispatchCount).toBe(0); expect(stopReceipt.all_terminated).toBe(true);`.
- [ ] Run `bun test packages/adapters/test/hermes-permissions.test.ts`; expected initially FAIL.
- [ ] Use actual `hermes acp` (v0.19.0); imports passed `hermes acp --check` during planning, but sessions/tool approvals have not been qualified. Map native permission/continuation events through the shared boundary and isolate profiles. Do not use one-shot `-z/--oneshot` (auto-bypasses approvals), `--accept-hooks`, browser setup/download or gateway activation. If an ACP operation lacks a supported callback, implement the missing native extension against pinned source and test its sink; do not replace it with a terminal mock or downgrade to unsafe headless mode.
- [ ] Run tests then `bun qa/smoke/harness.ts --harness hermes --connection <qualified-id>` for actual edit/deny/error/resume/cancel, confirming shared JEV runtime/model untouched.
- [ ] After smoke document qualification/profile behavior; commit `feat: add approval-preserving Hermes adapter`.

## Task H6: Roles, skills, scoped tools and deterministic graph orchestration

**Files:** Create core `src/scheduler/{graph,dispatch,budgets,handoffs}.rs`, `src/roles/{mod,registry,activation}.rs`, `src/tools/{mod,mcp,registry}.rs`; adapter `src/roles/{render,omp,codex,opencode,claude,hermes,alphacode}.ts`, `src/tools/mcp.ts`; tests core `tests/{job_graph,role_authority}.rs`; smoke `qa/smoke/cross-harness.ts`.

**Interfaces:** `GraphValidator::validate(graph: &JobGraph, policy: &ProjectPolicy) -> Result<ValidatedGraph>`; `Scheduler::submit(graph: ValidatedGraph, authority: TaskAuthority) -> Result<JobId>`; `RoleRegistry::activate(definition: RoleDefinition, preview_digest: Digest) -> Result<String>`; `ToolRegistry::for_task(run: RunId) -> Result<Vec<ToolDescriptor>>`; `RoleDefinition {id,instructions,required_capabilities,allowed_harnesses,allowed_routes,source_scope,tool_scope,delegated_limits}`; `JobGraph {id,project_id,version,nodes: JobNode[]}`; `JobNode {id,role_id,dependencies,route_pin?,allowed_routes,inputs,acceptance,workspace_mode}`. Handoff uses master ContextPackage, never hidden reasoning or fake native resume.

- [ ] Add tests `cycle_rejected_before_dispatch`, `failed_dependency_blocks_consumers_not_siblings`, `max_two_workers_queues_third`, `reviewer_cannot_write`, `skill_or_mcp_cannot_expand_policy`, `user_pin_is_not_semantic_preference`. Assert `assert_eq!(cycle_dispatch_count, 0); assert_eq!(running_workers, 2); assert_eq!(third_state, RunState::Queued);` and read-only review has no writer lease.
- [ ] Run `cargo test -p ironbark-core --test job_graph --test role_authority`; expected initially FAIL.
- [ ] Implement planner/researcher/implementer/reviewer/verifier plus user specialists. Generate native schema-specific agent definitions into owned per-harness profiles with import/activation preview; no shared frontmatter symlink and no overwriting user/native agents. Skills/hooks/project rules retain hash/source/activation provenance; allow execution only in existing task policy. Use actual qualified local stdio MCP and explicitly approved remote endpoint connectors with names/schema/auth/approval behavior; listing a tool is not permission to invoke it.
- [ ] Implement deterministic DAG validation, ready queue, route/capability/pin/budget checks, one writer, default configurable concurrency 2 and dependency failures. Surface observable native children; opaque inner work stays bounded under the parent. Results require native artifacts plus independent reviewer/verifier receipts; node success never implies all siblings succeeded. Scope context handoffs through P4 before a second harness/provider sees them.
- [ ] Run tests then `bun qa/smoke/cross-harness.ts`: real OMP implement, Codex or OpenCode read-only review, independent contained verifier command, one intentionally failing sibling, exact approved integration. Expected native cross-engine sessions, actual tests/diff evidence, failed dependent blocked, healthy sibling complete, only reviewed digest integrated. No external publication in this smoke.
- [ ] After smoke document role/tool/graph semantics; commit `feat: orchestrate scoped native roles and dependencies`.

## Task H7: AlphaCode permission extension and genuine adapter

**Files:** Create `integrations/alphacode/{source.json,build.sh,NOTICE,patches/0001-ironbark-permission-boundary.patch}`; pinned checkout `.sources/alphacode/` (ignored); adapter `src/harness/alphacode.ts`, `src/native/alphacode.ts`; test `test/alphacode-boundary.test.ts`; smoke qualification `qa/smoke/alphacode.ts`. Patch upstream `src/alphacode_tool_core/mod.rs`, `src/alphacode_app_core/tool/mod.rs`, `src/alphacode_protocol/wire.rs`, `src/alphacode_app_core/server/headless.rs`, `src/cli/acp.rs`, `src/cli/args.rs`; add upstream `src/alphacode_app_core/ironbark_permissions.rs`, tests and module wiring.

**Interfaces:** Implements `HarnessAdapter`; native extension `initialize._ironbark { permissionsVersion: 1, buildDigest, effectiveRoute, cwd, allowedTools }`; ACP `session/request_permission`/response correlates an ActionSpec and native request. Patch adds `PermissionGate::check(&self, action: NativeAction) -> Future<Result<PermissionReceipt>>` and a gate reference in ToolContext copied into subcalls; `NativeAction {session_id,tool_call_id,resolved_name,canonical_input,cwd,route,policy_version,state_digest}`. Missing gate in Ironbark-managed mode is an error, not allow. Native state/lease material never comes from an LLM argument.

- [ ] Add actual upstream regression tests `registry_denial_precedes_every_sink`, `direct_execution_cannot_skip_gate`, `child_context_keeps_gate`, `missing_reply_or_disconnect_denies`, `gate_uses_final_normalised_input`; adapter tests `unpatched_binary_rejected`, `existing_daemon_route_mismatch_rejected`, `text_replace_not_appended`, `cancel_observes_daemon_children`. Assert `assert_eq!(protected_file_after, protected_file_before);` and `expect(finalText).toBe(nativeReplacementText);` after denied/direct/child calls.
- [ ] Run `cargo test --manifest-path .sources/alphacode/Cargo.toml ironbark_permission` and `bun test packages/adapters/test/alphacode-boundary.test.ts`; expected initially FAIL.
- [ ] Pin commit `2a1d17156282e59fa811ceb58d9e28302d360fba`, preserve MIT/dependency notices, record source/build/checksum. Use LSP references for ToolContext, Registry::execute and direct Tool::execute before edits; route every managed execution caller through the same gate. Existing Registry::execute normalises input and ultimately calls `tool.execute(input.clone(), ctx.clone())`; gate final resolved/normalised parameters immediately before that sink. Its optional pre_tool hook is not sufficient if absent or fail-open. Add permission request/reply through native daemon to ACP, fail closed on timeout/disconnect, and bind consumed receipts to exact calls. Disable auto-attach to unrelated daemon and require reported route/cwd/tool set to match task on open/resume.
- [ ] Add a new explicit `--ironbark-managed` argument in the maintained patch. In this mode require the permission extension and dedicated owned socket; force no update checks/automatic updates, no self-development, telemetry off and disabled swarm. Launch dedicated daemon+ACP in C3 namespace with separate config/state, explicit provider/model/profile, `ALPHACODE_NO_TELEMETRY=1` and exact tool allowlist; no free/auto route. Do not assume the upstream boolean `--auto-update` accepts a `=false` value. Disable MCP/browser/desktop until authorised adapters exist. Require extension build/version digest; native sessions/tools remain AlphaCode's implementation.
- [ ] Run regression/adapter tests, then `bun qa/smoke/alphacode.ts` with an approved route: real text/edit/tool failure/denied path/stale grant/native resume/cancel and outbound-deny capture. Expected actual artifact, no denied effect, no telemetry/update/free-provider traffic and no remaining daemon/children. A build or sink gap blocks AlphaCode availability, not permission to leave a stub.
- [ ] After smoke document maintained patch surface/upgrade qualification and licenses; commit `feat: add policy-enforced AlphaCode native worker`.

## Task H8: Bounded browser and Linux desktop automation

**Files:** Create core `src/automation/{mod,broker,targets,consent}.rs`; adapter `src/tools/automation.ts`; add `integrations/alphacode/patches/0002-action-scoped-automation.patch`; patch upstream `src/alphacode_app_core/tool/browser.rs`, `tool/desktop/backend.rs`, `tool/desktop/mod.rs`, CLI args/dispatch; create upstream `src/alphacode_app_core/ironbark_automation.rs`; test core `tests/automation_authority.rs`; smoke `qa/smoke/automation.ts`.

**Interfaces:** `AutomationBroker::observe(target: ApprovedTarget) -> Result<AutomationObservation>`, `execute(action: ActionSpec, grant: ApprovalGrant) -> Result<AutomationReceipt>`, `release(scope_id: String) -> Result<()>`; `ApprovedTarget {scope_id,kind: BrowserProfile | NativeWindow,target_id,expires_at}`; `AutomationObservation {target_id,revision_digest,accessibility_tree?,image_artifact?,backend}`; receipt records actual observed outcome, not intended click. Consumes C2/C3 and H7 enforcement.

- [ ] Add `stale_snapshot_cannot_receive_click`, `page_instruction_cannot_approve_upload`, `coding_worker_cannot_access_session_bus`, `unsupported_wayland_input_returns_unsupported`, `disconnect_releases_authority`; assert `assert_eq!(unauthorised_effect_count, 0); assert_eq!(unsupported_action_error, CoreError::UnsupportedCapability);` and no send/upload/cookie/desktop change occurs without fresh scope.
- [ ] Run `cargo test -p ironbark-core --test automation_authority`; expected initially FAIL.
- [ ] Add an action-scoped native automation mode to the pinned AlphaCode build that executes its actual audited browser/AT-SPI implementations without running an agent/provider/swarm. Core launches a separate short-lived automation executor; coding workers receive only broker requests, never host session bus or personal browser access. Broker checks task scope, fresh target digest and exact grant before granting only the authorised operation; screenshots/clipboard/file upload are disclosures with explicit scope. Do not give broad D-Bus access to an arbitrary shell worker. Validate backend arguments and preserve real errors/results.
- [ ] Use a dedicated Firefox profile, audited extension/native-messaging registration and core-owned bridge process; extension/permission installation requires separate owner approval. Browser tab/session pins are target identity, not consent. Page-world eval, cookies, forms, uploads, purchases and publishing demand exact action scope; provider challenges/consent remain interactive, never automated bypass. Desktop target is an owner-selected window; use Linux AT-SPI accessible actions. Unsupported Wayland canvas/global-input behavior stays unavailable with explicit reason; no unrestricted shell fallback.
- [ ] Run tests, then `bun qa/smoke/automation.ts` under real KDE/Wayland with human presence: observe a disposable native editor, type approved harmless text, deny a save/send action, cancel and confirm authority released. Exercise private Firefox navigation/read/screenshot, deny upload and verify private profile/cookie isolation. Fresh screenshot/accessibility evidence after each change; separately qualify X11 only if advertising it. Never click external side-effect controls to prove denial.
- [ ] After smoke document real backend/action support, setup consent and emergency stop; commit `feat: add action-scoped browser and desktop assistance`.
