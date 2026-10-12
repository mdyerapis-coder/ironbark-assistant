import type { CandidateCodes, DecodedChoice, ScoreEvidence } from "./types";

export function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** No trimming, case-folding, prefix matching, JSON extraction or defaults. */
export function decode(raw: unknown, candidates: readonly string[] | CandidateCodes): DecodedChoice {
  if (typeof raw !== "string" || raw.length === 0) return { state: "invalid" };
  if (raw === "ABSTAIN") return { state: "abstain" };
  if (Array.isArray(candidates)) {
    return candidates.includes(raw) ? { state: "available", choice: raw } : { state: "invalid" };
  }
  const mapping = candidates as CandidateCodes;
  return Object.hasOwn(mapping, raw) && typeof mapping[raw] === "string"
    ? { state: "available", choice: mapping[raw] }
    : { state: "invalid" };
}

/** b11559 native nonstreaming /completion, not the facade's decision JSON. */
export function parseCompletion(rawJson: string): Record<string, unknown> | null {
  let value: unknown;
  try { value = JSON.parse(rawJson); } catch { return null; }
  if (!object(value) || typeof value.content !== "string" || value.stop !== true ||
      value.truncated !== false || value.stop_type !== "eos" ||
      typeof value.model !== "string" || !value.model) return null;
  return value;
}

export function decodeCompletion(rawJson: string, candidates: CandidateCodes): DecodedChoice {
  const value = parseCompletion(rawJson);
  return value ? decode(value.content, candidates) : { state: "invalid" };
}

/**
 * A sampled token's pre-sampling log probability, never label confidence.
 * n_probs/top_logprobs do not establish a calibrated selective error bound.
 * Missing, post-sampling-only, corrupt or unaligned evidence stays unknown.
 */
export function scoreEvidence(value: unknown): ScoreEvidence {
  if (!object(value) || typeof value.content !== "string" ||
      !Array.isArray(value.completion_probabilities) || !value.completion_probabilities.length)
    return { type: "none" };
  const first: unknown = value.completion_probabilities[0];
  if (!object(first) || typeof first.token !== "string" || !first.token ||
      !value.content.startsWith(first.token) || typeof first.logprob !== "number" ||
      !Number.isFinite(first.logprob) || first.logprob > 0) return { type: "none" };
  return { type: "raw_token_log_prob", value: first.logprob, token: first.token, token_position: 0 };
}
