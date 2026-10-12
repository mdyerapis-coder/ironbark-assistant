# Providers and judgment boundaries

Provider invocation happens in the Bun adapter worker, never in the core. The core
registers connections, owns credential leases, and decides policy; the adapter is a
transport that executes one pinned route and reports back. This document describes the
implemented boundary, not a live-provider qualification.

## Connection pinning

`ProviderRegistry.resolve` refuses a dispatch unless:

- the connection exists and its protocol matches the route,
- the route carries no `harness` override (a provider route is not a harness route),
- the connection id is in the policy's `allowed_connection_ids`.

Known SDK protocols map to fixed SDK API descriptors (`SDK_APIS`); a `custom`
protocol requires an explicit activation version, otherwise dispatch fails with
`custom_activation_required`. An unknown protocol is `unsupported_protocol`. There is
no registry-level fallback to another account, provider or protocol.

## Exact protocol, no fallback

The OMP-SDK adapter (`providers/omp-sdk.ts`) enforces the transport contract:

- **Credential lease, not key.** The adapter receives a `credential_lease_id` and
  resolves bytes through a `CredentialResolver` on the private fd 3 binary lease
  channel. Raw keys never appear in the route, the worker environment or a log. An
  empty or missing credential resolves to a no-auth sentinel rather than an implicit
  account.
- **One dispatch per attempt.** On the first failed request the adapter aborts the
  dispatch; `providerRetryWait` throws `retry_requires_core_dispatch`. The SDK's
  separate transport-retry layer is disabled by construction. Only the core may
  re-dispatch, under a fresh request identity.
- **Controller composition.** The adapter's `AbortController` is composed with the
  SDK's derived watchdog signal (init or request signal) via `AbortSignal.any`, so
  owner cancellation and the first-event watchdog both abort the fetch; replacing
  one with the other is a bug the boundary exists to prevent.
- **Output ceilings.** Completed-line and cumulative upstream/output/producer
  backlog limits are enforced on the streamed result; a ceiling breach aborts the
  turn as failed rather than unbounded growth.
- **Exact replay.** A native replay is matched one-to-one against canonical
  `tool_calls`, including JSON arguments; a mismatched replay is a protocol error,
  not a silent drop.

A turn ends in exactly one of `succeeded`, `failed`, or `cancelled` — the distinction
between an owner cancellation and a provider failure is preserved in the terminal
event.

## JEV judgment boundary

The local JEV client (`jev/client.ts`) is a judgment **transport**, not a judge:

- **Strict raw decoding.** Responses are decoded from raw JEV frames; undecodable or
  truncated output is an error, never a coerced label.
- **Explicit abstention.** The client preserves the JEV's own abstention state. A
  judgment the model does not make is reported as unavailable, not defaulted.
- **Abort on every exit.** Every exit path (cancel, timeout, error, success) aborts
  the in-flight operation controller and clears siblings; the abort reason is
  captured before the `finally` re-asserts abort, so the terminal state is honest.
- **Metadata-only identity.** Client metadata identifies the endpoint; it does not
  qualify a semantic domain. Domain qualification requires held-out
  positive/negative/ambiguous cases, which this client does not perform.

## Observed verification

63 Bun tests pass across `provider-boundaries.test.ts` (31) and `jev-decode.test.ts`
(29): protocol pinning, credential-lease resolution, first-failure abort, no-fallback
behaviour, output ceilings, exact replay matching, strict JEV decoding and
abstention preservation. These are boundary tests against fixtures and transport
shapes. **No live provider account, native harness or JEV domain is qualified.**
Live qualification requires owner-approved connections and is outstanding.
