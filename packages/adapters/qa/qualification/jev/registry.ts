// qa/qualification/jev/registry.ts — shared loading/identity helpers for the J2
// qualification scripts.
//
// Fixture rows are validated exactly as the domain builder validates them
// (content_digest must equal sha256(text)); a raw JSON line is never cast into a
// FixtureRow. Judgment decision ids are projected into UUID form because the core
// contract types DecisionId as a UUID, and the projection is deterministic so a
// replayed row keeps the same identity. The observed runtime identity is read in
// exactly one place, so a calibration receipt and an evaluation report bind to
// the same digest.
import { loadJsonl, validateFixtureRows } from "../../../src/jev/domains/types";
import type { DomainDefinition, FixtureRow } from "../../../src/jev/domains/types";
import { FACADE_BASE, RUNTIME_BASE, lexicalJson, sha256 } from "../../../src/jev/identity";
import type { JevTransport, RuntimeIdentity } from "../../../src/jev/types";

export async function loadFixtureRows(domain: DomainDefinition): Promise<FixtureRow[]> {
  const root = process.env.IRONBARK_ROOT ?? process.cwd();
  const rows: FixtureRow[] = [];
  for (const f of domain.fixtures) {
    const text = await Bun.file(resolveFixture(root, f)).text();
    rows.push(...validateFixtureRows(loadJsonl(text)));
  }
  return rows;
}

/**
 * Offline transport for structural runs. It answers every facade/runtime route
 * with a non-identity payload, so the client reports `unqualified` instead of
 * inventing a choice; no live model is contacted.
 */
export const offlineTransport: JevTransport = () =>
  Promise.resolve(new Response('{"state":"abstain","choice":"ABSTAIN"}', {
    headers: { "content-type": "application/json" },
  }));

/**
 * Runtime identity as reported metadata only: the facade must answer /health and
 * the raw runtime must report a model alias, path and build. The digest binds the
 * live runtime to a pre-registered claimed digest; an unreachable runtime, or one
 * that reports no model identity at all, is null and MUST be treated downstream
 * as an identity mismatch (never as a passing identity).
 */
export async function probeRuntimeIdentity(transport: JevTransport): Promise<RuntimeIdentity | null> {
  try {
    const health = await transport(`${FACADE_BASE}/health`, { method: "GET", signal: AbortSignal.timeout(3000) });
    if (!health.ok) return null;
  } catch {
    return null;
  }
  try {
    const props = await transport(`${RUNTIME_BASE}/props`, { method: "GET", signal: AbortSignal.timeout(3000) });
    if (!props.ok) return null;
    const j: Record<string, unknown> = await props.json();
    // model_alias is the field the runtime's /props actually reports (see
    // src/jev/identity.ts); the remaining names are older aliases.
    const model_identity = String(j.model_alias ?? j.model_name ?? j.model_identity ?? j.model ?? "");
    if (!model_identity) return null;
    const model_path = String(j.model_path ?? j.path ?? "");
    const build_info = String(j.build_info ?? j.server ?? JSON.stringify(j));
    return { digest: sha256(lexicalJson({ model_identity, model_path, build_info })), model_identity, model_path, build_info, assurance: "reported_metadata_only" };
  } catch {
    return null;
  }
}

/**
 * Deterministic decision id for a judgment label. A judgment's decision_id is a
 * UUID in the core contract, so a fixture id cannot be used directly; the same
 * label always yields the same id, keeping replays idempotent.
 */
export function decisionIdFor(label: string): string {
  const nibbles = sha256(`ironbark-jev:decision-id:v1:${label}`).slice(0, 32).split("");
  nibbles[12] = "4"; // version 4
  nibbles[16] = "8"; // RFC 4122 variant
  const hex = nibbles.join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

function resolveFixture(root: string, f: string): string {
  if (f.startsWith("http://") || f.startsWith("https://") || f.startsWith("file://")) return f;
  return root.endsWith("/") ? root + f : root + "/" + f;
}
