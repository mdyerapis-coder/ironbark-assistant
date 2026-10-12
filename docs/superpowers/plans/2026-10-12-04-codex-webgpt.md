# Codex WebGPT Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect ChatGPT Web through the best inspected bridge while preserving genuine Codex authority, safe tools, account consent and honest qualification.

**Architecture:** Core supervises one pinned bridge/browser bundle and genuine native Codex worker per run. WebGPT is a special connection, not a generic OpenAI-compatible provider for every harness. Reuse the local OMP-to-Codex adapter only after its combined contract qualifies.

**Tech Stack:** `miuuyy/codex-chatgpt-web` commit `0b6087c5307c7a58adda9c420358f3f70d2035f5` (package 6.1.8), native Codex 0.162.1, Bun, contained private browser, typed core events; selected MIT receipt semantics from `chicogong` commit `0781dc2f18d853b77c546d5d59c6bed1243c5c47`.

**Spec:** [design](../../design/DESIGN.md) §6; [comparison and license decisions](../../design/WEBGPT-COMPARISON.md); [master](2026-10-12-ironbark-assistant.md).

## Global Constraints

- Use one canonical bridge and one application adapter; no wholesale repository merge.
- Native provider/website consent remains a human decision.
- No fallback to another API route when the Web connection fails.
- Keep connector approval distinct from native filesystem/process approval.
- Ordinary upstream Responses requests lack a bridge-specific bearer secret; an authenticated facade alone does not secure a directly reachable unauthenticated backend.
- No existing native configuration or browser authentication store is overwritten.

## Review Focus

1. Raw Responses or browser-debug listener reachable by an unrelated process: reject qualification (W1).
2. Wrong account/model, missing connector permission or global route change: refuse turn without fallback (W1/W2).
3. Ambiguous send/timeout: reconcile, never send another prompt automatically (W3).
4. Denied native tool, ChatGPT consent or unsupported field: no fabricated result/escalation (W2).
5. Cross-project native history/image/cancellation leakage: distinct session/profile authority and targeted stop (W2/W3).

---

## Task W1: Pinned sidecar, isolated ownership and activation state

**Files:** Create `integrations/webgpt/{source.json,build.sh,managed-profile.json,NOTICE}`; adapter `src/webgpt/{lifecycle,config,status}.ts`; core `src/connections/webgpt.rs`; test `test/webgpt-boundary.test.ts`; smoke `qa/smoke/webgpt-boundary.ts`.

**Interfaces:** `WebGptConnection.describe() -> Promise<ConnectionDescriptor>`; `start(input: WebGptLaunchInput) -> Promise<WebGptLease>`; `stop(lease: WebGptLease) -> Promise<StopReceipt>`; `WebGptLaunchInput {run_id,project_id,managed_profile_id,route,approved_activation_digest?}`; `WebGptState = stopped | starting | auth_needed | connector_approval_needed | ready | unavailable | stalled`. Consumes C2/C3 and H2's native client. Leases/config authentication remain native-private.

- [ ] Add `unrelated_local_peer_cannot_reach_raw_bridge`, `global_codex_config_is_unchanged`, `identity_mismatch_blocks_ready`, `missing_login_is_auth_needed_not_fallback`. Assert `expect(unrelatedPeerSubmittedTurns).toBe(0); expect(globalConfigAfter).toEqual(globalConfigBefore); expect(missingLoginState).toBe("auth_needed");`.
- [ ] Run `bun test packages/adapters/test/webgpt-boundary.test.ts`; expected initially FAIL.
- [ ] Pin/build exact upstream source and dependencies; no moving `main`, generated-bundle regex patch, broad pkill or alternate replay bridge. Dedicated browser profile and Codex config/auth/rollout scope. Upstream expects built-in `openai` identity and can install route/Interrupt hook settings: own only managed copies, validate them and prove native authority. Define one lifecycle owner: core owns the launcher bundle, launcher owns its browser/native children; core records/contains that tree rather than separately starting the same native worker.
- [ ] Place raw Responses/browser-debug listeners inside a dedicated network boundary accessible only through authorised core/worker channels; protect facade/admin routes with independent random credentials and correct Origin/Host/request limits. Test the raw backend too, not just facade. External ChatGPT/tunnel endpoints require explicit approved egress; no wildcard network to make setup easier. Disable composer predictions; temporary chats `useSavedChats=false` unless owner opts in; connector autoapproval false. Verify actual pinned config keys during implementation.
- [ ] Implement activation preview describing exact ChatGPT Developer Mode, named `Codex Native2` connector, Authentication None / Allow all actions, outbound secure MCP tunnel, account/workspace entitlement and managed native hook. Point-of-risk interactive approval and applicable-terms review are mandatory; opening the preview is not consent. No account changes/login/challenges automated. Refusal leaves full mode unavailable, not mock-connected.
- [ ] Run boundary tests and `bun qa/smoke/webgpt-boundary.ts` without sending a provider turn: start managed isolated sidecar, observe actual health/auth state, attempt unrelated raw/admin access, stop and inspect children/config. Expected containment/denials and actual auth-needed if not logged in. Live activation occurs only after exact consent; it is a separately recorded gate.
- [ ] After smoke document boundary/residual trusted-client model and activation steps; commit `feat: supervise isolated Codex WebGPT connection`.

## Task W2: Genuine native turns, image input and qualified executor tools

**Files:** Create `src/webgpt/{native-route,events,qualification}.ts`; create `src/webgpt/omp-native.ts` by adapting audited local donor `~/Projects/omp-webgpt-native-adapter/src/{index,native-client,session}.ts` into the new owned package; test `test/webgpt-native.test.ts`; smoke `qa/smoke/webgpt-roundtrip.ts`.

**Interfaces:** `WebGptNativeAdapter` implements `HarnessAdapter` for the qualified Codex route; `OmpWebGptAdapter` delegates genuine Codex model/session work while OMP owns actual dynamic tools and approvals. `qualifyWebGpt(input: WebGptQualificationInput) -> Promise<QualificationReport>`; report contains exact source/native/SDK/schema identities and separate capability states.

- [ ] Add `unknown_native_approval_is_denied`, `chat_only_has_no_executable_tools`, `failed_native_tool_is_failed_not_text_success`, `image_payload_is_actual_upload_not_filename`, `project_session_isolation`. Assert `expect(deniedFileExists).toBe(false); expect(session.native_id).toBe(nativeServerThreadId); expect(toolFreeExecutionCount).toBe(0);`; unsupported escalation fields fail.
- [ ] Run `bun test packages/adapters/test/webgpt-native.test.ts`; expected initially FAIL.
- [ ] Bind real upstream/native protocol, not synthetic Responses IDs. Generate current Codex schemas and coordinate bridge/SDK 18.8.9 update with native dynamic-tool tests; historical adapter 18.6.1 is not proof of compatibility. Primary core-to-Codex route retains Codex tools. Optional OMP route uses OMP native executor/dynamic tool output and no fabricated Codex session. Qualify both separately; never register WebGPT as a generic OMP OpenAI-compatible connection. Make account/model inventory evidence-backed rather than a historical hardcoded list.
- [ ] Map genuine streaming, native history/compaction, actual image upload/provenance, tool output/errors and denial. Browser consent remains interactive and separate from core grants. Only advertise supported image/audio/generation capabilities after real evidence; source image input does not prove an image-generation API. Native completion must carry actual output/artifacts and acceptance evidence.
- [ ] Run tests; after activation approval run `bun qa/smoke/webgpt-roundtrip.ts --route codex-native`, then `--route omp-native` if selected. Exercise real text, distinguishable image input, two isolated sessions, actual bounded temporary-file tool, denied write, explicit native error and user-cancelled turn. Expected actual account/model/native IDs, image understanding, honest error/denial and exact workspace effects. Record every capability separately; absence of credentials/entitlement blocks this required integration gate, not license to mark it passed.
- [ ] After smoke document actual supported model/route matrix and source notices; commit `feat: preserve native WebGPT turns and executor ownership`.

## Task W3: Submission receipts, duplicate-send defense and recovery

**Files:** Create core `src/execution/{submission,receipts}.rs`; adapter `src/webgpt/recovery.ts`; test core `tests/webgpt_submission.rs`; smoke `qa/smoke/webgpt-recovery.ts`; modify checkpoints/events.

**Interfaces:** `SubmissionStore::prepare(run: RunId, session: SessionRef, prompt_digest: Digest, scope_digest: Digest) -> Result<SubmissionRecord>`; `reconcile(record_id: String, observation: SubmissionObservation) -> Result<SubmissionOutcome>`; outcomes `NotSubmitted | Submitted | Completed | Ambiguous | Failed`. Consumes C1/C4 native observation/checkpoint state, not generic timeout heuristics.

- [ ] Add `same_request_after_reconnect_does_not_resend`, `timeout_after_send_is_ambiguous`, `receipt_hash_is_not_authorisation`, `completion_without_artifact_is_not_accepted`. Assert `assert_eq!(outcome, SubmissionOutcome::Ambiguous); assert_eq!(automatic_resubmissions, 0);` until actual reconciliation evidence.
- [ ] Run `cargo test -p ironbark-core --test webgpt_submission`; expected initially FAIL.
- [ ] Port only audited donor fingerprint/preflight/route-evidence/reconciliation/receipt-commitment semantics into the existing store; adapt mode families deliberately. Preserve MIT attribution if copying code. Receipt SHA-256 commits exact inputs/scope/results, not identity signatures or truth. Keep private metadata under 0700/0600; never add donor scheduler or trust its backend-presence heuristics.
- [ ] Implement targeted abort across HTTP, browser and native hook/process ownership, verifying actual stop. On uncertain prompt submission retain native observations and ask for reconciliation; never retry by opening a fresh chat or fall back to another API account.
- [ ] Run tests; after live approval `bun qa/smoke/webgpt-recovery.ts` interrupts one real turn around submission/streaming, restarts the managed core/sidecar and reconciles. Expected no duplicate browser prompt, preserved native history/partial artifacts, explicit unknown outcome if evidence is insufficient and owned descendants stopped. Use harmless content, not an external action.
- [ ] After smoke update WebGPT recovery/receipt docs; commit `feat: reconcile WebGPT submissions without duplicate sends`.
