# Connections, Conversations and Knowledge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Connect the user's providers to genuine streamed conversations, branching/history, attachments and project-scoped cited knowledge/memory.

**Architecture:** Core validates route/disclosure and commits turns before a supervised Bun SDK adapter makes requests. Local immutable source revisions, SQLite FTS5 and optional qualified local embeddings serve retrieval. Native credential references never become frontend/prompt fields.

**Tech Stack:** OMP `@oh-my-pi/pi-ai` 18.8.9, Bun/TypeScript, Rust/SQLite FTS5, Secret Service/explicit Bitwarden refs, local `pdftotext`, Pandoc, Tesseract and qualified transcription adapters.

**Spec:** [design](../../../DESIGN.md) §§4,7,10; [master](2026-10-12-ironbark-assistant.md).

## Global Constraints

- No claim that every provider can run through every harness.
- Credentials stay outside the frontend, prompts, retrieval index, logs, and exports.
- No silent fallback to a different account/provider/model.
- Sources, project memory and working context remain distinct.
- Apply project/access/disclosure filtering before ranking and before constructing context.
- Updating a source does not rewrite old conversation citations.

## Review Focus

1. Outage, absent capability or unknown billing: no fallback or false free/support label (P1).
2. Interrupted SSE, repeated Send, edit/regenerate: preserve partial content and distinct branches without replaying tools (P2).
3. Malformed/encrypted documents and media without qualified extractor: fail truthfully, no fabricated text (P3).
4. Cross-project/private chunk, stale/deleted revision: filter before disclosure and open exact historic provenance (P4).
5. Proposed memory/instructions, unknown embedding revision: no automatic promotion or wrong-vector ranking (P5).

---

## Task P1: Provider registry, native secrets and qualified protocol streams

**Files:** Create core `src/connections/{mod,registry,credentials,qualification,imports}.rs`; adapter `packages/adapters/{package.json,src/main.ts,src/rpc.ts,src/providers/{registry,omp-sdk,local,custom}.ts}`; modify core contracts/IPC; test `packages/adapters/test/provider-boundaries.test.ts`, core `tests/provider_policy.rs`; smoke `qa/smoke/provider-roundtrip.ts`.

**Interfaces:** Consumes C1/C2 contracts. Produces master `ProviderAdapter`; `ConnectionRegistry::register(input: ConnectionConfig) -> Result<String>`; `select(pin: &RoutePin, policy: &ProjectPolicy, required: &[Capability]) -> Result<QualifiedRoute>`; `CredentialStore::resolve(reference: &AuthRef) -> Result<Zeroizing<Vec<u8>>>`; `AuthRef = SecretService {service,account} | Bitwarden {item_id,field} | NativeSession {harness,profile}`. `ProviderTurnInput { run_id, route, messages: ProviderMessage[], required_capabilities: Capability[], tool_schemas: ToolDescriptor[], disclosure_digest: Digest, credential_lease_id: string }`; lease IDs are private native-worker data, not UI events. `ConnectionDescriptor {id,protocol,endpoint,models,capabilities,availability,auth_state}`.

- [ ] Add tests `unsupported_tools_reject_before_request`, `outage_never_chooses_another_account`, `local_only_rejects_cloud_endpoint`, `credential_sentinel_is_absent_from_events`, `usage_without_billing_is_unknown`. Assert `expect(outboundDeniedRequests).toBe(0); expect(routeAfterOutage).toEqual(selectedRoute); expect(billing.state).toBe("unknown");` without provider billing evidence.
- [ ] Run `bun test packages/adapters/test/provider-boundaries.test.ts` and `cargo test -p ironbark-core --test provider_policy`; expected initially FAIL.
- [ ] Implement the interfaces using OMP SDK's real protocol adapters for Chat Completions, Responses, Anthropic and Gemini; support Ollama native/local OpenAI-compatible and llama.cpp endpoints with actual negotiated behavior. Reuse SDK image/audio/embedding/transcription exports only where that route qualifies; custom adapters implement the same contract and are explicitly activated as code, not inferred from URL shape. Pin SDK 18.8.9 and Bun lockfile. Unknown catalog capabilities remain unknown until a safe probe qualifies them.
- [ ] Implement core credential resolution via Secret Service or an explicitly selected Bitwarden item; native login remains harness-owned. Zeroise short-lived secret material, scrub inherited environment and keep credentials out of JSON configuration/export. Import KeyHarbor catalog/auth-reference snapshots through an additive preview `preview_keyharbor(snapshot: KeyHarborSnapshot) -> ImportPreview`; map protocol/model/ref fields, never run its global coordinator or scrape private auth databases. Native configuration writes require exact preview approval.
- [ ] Run focused tests and `bunx tsc --noEmit`; expected pass. Implement/run `bun qa/smoke/provider-roundtrip.ts --connection <owner-approved-id>` through the actual core, first against local llama.cpp, then each approved configured protocol family. Check actual streamed answer, effective route, abort, error and real usage observations. Paid or private-context probes require explicit allowance. Each user's registered connection needs its own capability evidence before being selectable for a feature; unavailable credentials block that connection, not the registry implementation.
- [ ] After smoke document supported protocols/import/credential reference setup and actual qualified routes; commit `feat: connect qualified providers without route fallback`.

## Task P2: Durable conversation tree and streamed turn lifecycle

**Files:** Create core `src/conversations/{mod,branches,turns,search,exports}.rs`; adapter `src/providers/events.ts`; modify contracts/IPC/CLI; test `tests/conversation_tree.rs`, adapter `test/stream-events.test.ts`; smoke `qa/smoke/conversation-tree.ts`.

**Interfaces:** `ConversationService::create(project: ProjectId) -> Result<ConversationId>`; `start_turn(conversation: ConversationId, branch: BranchId, request_id: Uuid, parts: Vec<ContentPart>, pin: RoutePin) -> Result<RunId>`; `edit(message: MessageId, parts: Vec<ContentPart>) -> Result<BranchId>`; `regenerate(message: MessageId, pin: RoutePin) -> Result<BranchId>`; `archive(id: ConversationId) -> Result<()>`, `delete(id: ConversationId, run_resolution: RunResolution) -> Result<()>`. Consumes `ProviderAdapter`, core event/request identity and C4 state (P2 can implement turn persistence before C4; cancellation acceptance waits for C4).

- [ ] Add tests `edit_creates_new_branch_and_preserves_old_receipts`, `regeneration_is_sibling_not_overwrite`, `interrupted_stream_preserves_partial_without_success`, `reconnect_does_not_repeat_turn`, `native_text_replace_replaces_one_message`. Assert `assert_ne!(edited_branch, original_branch); assert_eq!(old_receipts, before_edit_receipts);` and `expect(finalText).toBe(replacementText);` rather than concatenated deltas.
- [ ] Run `cargo test -p ironbark-core --test conversation_tree` and `bun test packages/adapters/test/stream-events.test.ts`; expected initially FAIL.
- [ ] Implement source/attachment provenance, searchable rename/archive/history, parent-linked immutable branches, per-turn route/context disclosure snapshots and event projections. Editing never rolls back past tool effects; show that explicitly. Send commits request/run identity before provider dispatch. Preserve partial response on abort/error, native tool/result/error distinctions, and unknown usage. Exports are owner-initiated redacted JSON/Markdown with citations; secrets and raw native auth are excluded.
- [ ] Run tests; then `bun qa/smoke/conversation-tree.ts --connection <approved-local-id>` creates a live conversation, interrupts streaming, reconnects, edits and regenerates. Expected live answers on separate branches, preserved partials, original receipt intact and no duplicate provider turn on reconnect. Resolving archive/delete while active must stop/reconcile runs before content removal.
- [ ] After smoke update conversation operations docs/changelog; commit `feat: persist streamed conversations and branch history`.

## Task P3: Immutable source ingestion and real extraction

**Files:** Create core `src/knowledge/{mod,sources,extractors,locators}.rs`; adapter `src/media/{ocr,transcription}.ts`; test `tests/source_revisions.rs`; fixtures `qa/fixtures/{source-v1.md,source-v2.md,encrypted.pdf,malformed.docx}`; smoke `qa/smoke/source-ingest.ts`.

**Interfaces:** `KnowledgeService::ingest(project: ProjectId, input: SourceInput, disclosure: DisclosurePolicy) -> Result<SourceRevision>`; `SourceInput = File {path} | Attachment {artifact_id} | Fetched {url,receipt}`; `SourceRevision {source_id,project_id,digest,mime,extractor_id,extractor_version,locators}`; `CitedExcerpt {citation: Citation,text,project_id,disclosure: DisclosurePolicy}`; `DisclosurePolicy = LocalOnly | AllowedConnections {connection_ids} | ExplicitPublic`; `CitationResolver::open(citation: &Citation) -> Result<CitationView>`, where `CitationView = Content {text,locator} | Deleted | Missing`.

- [ ] Add tests `source_update_keeps_old_citation_exact`, `encrypted_pdf_is_not_indexed_as_blank_success`, `extractor_failure_publishes_no_partial_revision`, `remote_extraction_obeys_disclosure`, `oversized_attachment_has_no_committed_object`. Assert `assert_eq!(old_citation_text, v1_text); assert_eq!(failed_source_searchable_chunks, 0);` after v2 ingestion and extractor failure.
- [ ] Run `cargo test -p ironbark-core --test source_revisions`; expected initially FAIL.
- [ ] Implement plain text/Markdown/code line locators; HTML sanitised text/section anchors; PDF via `pdftotext` with page boundaries; DOCX/ODT/RTF via Pandoc with paragraph/section locators; XLSX/ODS via Rust `calamine` with master sheet/cell locators. Detect encrypted/malformed/unsupported content explicitly. Validate extractor outputs and versions before atomic revision commit. Extractors run as C3 read-only workers with size/time limits, not arbitrary executable documents.
- [ ] Implement qualified local OCR (Tesseract page/box provenance) and audio transcription through local runtime or explicitly approved provider. Gate these capabilities on actual tool/model availability; never return made-up media contents or upload LocalOnly sources. Install required local extractors during execution with native package consent; record versions and licenses, not planning-time installation.
- [ ] Run source tests; `bun qa/smoke/source-ingest.ts` ingests actual Markdown, PDF, HTML, DOCX, XLSX, image and short audio fixtures through the core. Expected exact known fixture content/locators, preserved revision updates and honest failure on encrypted/malformed files. Record media qualified/unavailable separately; every advertised media function requires its real extractor smoke.
- [ ] After smoke document MIME support/provenance/limits and extraction disclosure; commit `feat: ingest immutable cited knowledge sources`.

## Task P4: Filter-before-ranking retrieval and deletion semantics

**Files:** Create core `src/knowledge/{chunks,retrieval,deletion,context}.rs`; modify migrations/IPC/CLI; test `tests/retrieval_disclosure.rs`; smoke `qa/smoke/retrieval-disclosure.ts`.

**Interfaces:** `KnowledgeService::retrieve(project: ProjectId, query: &str, route: &RoutePin, limit: usize) -> Result<Vec<CitedExcerpt>>`; `build_context(project: ProjectId, route: &RoutePin, brief: &str, citations: &[Citation]) -> Result<ContextPackage>`; `delete_source(source_id: String) -> Result<DeletionReceipt>`.

- [ ] Add tests `private_chunk_cannot_reach_cloud_ranking_or_context`, `same_title_in_other_project_is_excluded`, `deleted_source_loses_chunks_and_vectors`, `historic_deleted_citation_returns_deleted`. Assert `assert!(!ranked_text.contains("private_sentinel")); assert!(!outbound_payload.contains("private_sentinel")); assert_eq!(historic_view, CitationView::Deleted);` rather than checking only UI text.
- [ ] Run `cargo test -p ironbark-core --test retrieval_disclosure`; expected initially FAIL.
- [ ] Implement project/access/disclosure SQL filtering before FTS rank; token/character budget packing retains locators. Recheck policy immediately before provider/tool dispatch so stale route/context selections cannot disclose. Delete active extraction chunks/vectors and revision content according to owner deletion scope; historical references resolve explicit Deleted/Missing, never silent current-revision substitution. Cancellation stops pending ingestion/index work for deleted sources.
- [ ] Run retrieval tests; `bun qa/smoke/retrieval-disclosure.ts` queries two real projects with identical titles and a unique LocalOnly sentinel, targets a local permitted route and a forbidden-cloud capture endpoint. Expected cited local answer and zero forbidden private payload; then delete source and reopen old citation to observe Deleted. Capture endpoint is a throwaway transport observation, not fake model-success evidence.
- [ ] After smoke update retrieval/deletion docs/changelog; commit `feat: enforce project-scoped cited retrieval`.

## Task P5: Approved memory and optional versioned local embeddings

**Files:** Create core `src/knowledge/{memory,embeddings,index_jobs}.rs`; adapter `src/providers/embeddings.ts`; test `tests/memory_embeddings.rs`; smoke `qa/smoke/memory-and-embeddings.ts`.

**Interfaces:** `MemoryService::propose(input: MemoryCandidate) -> Result<String>`, `decide(candidate_id: String, edited_text: Option<String>, decision: MemoryDecision) -> Result<()>`, `forget(memory_id: String) -> Result<()>`; `EmbeddingIndex::index(revision: SourceRevision, model: EmbeddingModelIdentity) -> Result<IndexReceipt>`; `EmbeddingModelIdentity {connection_id,model_id,version,dimensions}`. Consumes P3/P4 disclosure, C3/C5 budget/objects.

- [ ] Add tests `proposal_is_not_retrievable_before_approval`, `forget_removes_active_memory`, `retrieved_instruction_cannot_expand_tools`, `wrong_dimension_or_model_revision_is_rejected`, `embedding_outage_keeps_lexical_retrieval`. Assert `assert_eq!(unapproved_memory_hits, 0); assert_eq!(forgotten_memory_hits, 0);` and a lexical result is explicitly labelled lexical, with approved provenance intact.
- [ ] Run `cargo test -p ironbark-core --test memory_embeddings`; expected initially FAIL.
- [ ] Implement separate candidate/approved/forgotten records tied to originating evidence; owner edit/reject/approve is explicit, no cross-project sharing. Working summaries do not silently become memory. Implement optional local embeddings tagged by full model identity/dimensions/revision; only affected sources reindex, bounded to 2 indexing workers and C5 limits. Use SQLite-owned vector storage/local cosine ranking of authorised candidates; no mandatory external vector service. Reject incompatible vectors, preserve FTS fallback visibly.
- [ ] Run tests; `bun qa/smoke/memory-and-embeddings.ts` proposes an actual conversation fact, confirms unavailable until owner-approved, retrieves approved fact, forgets it, then exercises real local embedding retrieval with one model revision and an offline index route. Expected provenance intact, forgotten fact excluded, lexical fallback named. Optional embedding absence does not block chat/FTS, but semantic capability remains unavailable until qualified.
- [ ] After smoke document memory and optional indexing lifecycle; commit `feat: add explicit memory and qualified local embeddings`.
