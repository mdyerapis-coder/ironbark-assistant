#!/usr/bin/env bun
/**
 * J2 calibration fit (qa/qualification/jev/calibrate.ts).
 *
 * Fits a selective-risk abstention threshold on the CALIBRATION split for each
 * domain, using the REAL local JEV runtime. The threshold is never fitted on the
 * holdout split, and the receipt is bound to the exact dataset_digest and the
 * observed identity_digest — it is the J3 input and DOES NOT grant auto-select
 * authority by itself (canAutoSelect still requires a passing gate plus a
 * calibrator bound to this dataset and identity).
 *
 * Modes:
 *   --live       : hit the real facade+model (default).
 *   --offline    : no live model; every row is unqualified and the calibrator is
 *                  reported unavailable (no identity, no threshold).
 *   --domain <d> : one domain or `all` (default all).
 *
 * The calibrator is a simple monotone threshold on the model's raw first-token
 * log-probability. It selects the highest threshold such that the empirical error
 * rate on accepted choices in calibration does not exceed a target (default
 * 0.05). If no accepted choices exist at any threshold, the calibrator is marked
 * unavailable (threshold and error rate are null) instead of claiming a value.
 *
 * Output: one JSON line per domain — CalibratorReceipt.
 */
import { stdout, stderr, exit } from "node:process";
import { JevClient } from "../../../src/jev/client";
import { DomainRegistry } from "../../../src/jev/registry";
import type { JudgmentInput } from "../../../src/jev/types";
import {
  buildDataset,
  promptDigest,
  REQUIRED_CLASSES,
  type DomainDefinition,
  type FixtureRow,
} from "../../../src/jev/domains/types";
import type { JudgmentState } from "../../../../contracts/src/generated";
import { decisionIdFor, offlineTransport, probeRuntimeIdentity } from "./registry";

const args = process.argv.slice(2);
function flag(name: string): string | undefined {
  const i = args.indexOf(name);
  if (i < 0) return undefined;
  return args[i + 1];
}
const live = !args.includes("--offline");
const domainFlag = flag("--domain") ?? "all";

const ALL_DOMAINS = DomainRegistry.all();
const domainsToRun: DomainDefinition[] = domainFlag === "all"
  ? ALL_DOMAINS
  : ALL_DOMAINS.filter((d) => d.domain === domainFlag);
if (domainsToRun.length === 0) {
  stderr.write(`unknown judgment domain: ${domainFlag}\n`);
  exit(2);
}

export interface CalibratorReceipt {
  domain: string;
  dataset_digest: string;
  /**
   * Digest of the runtime identity the fit ran under. null when no identity was
   * observed: the receipt then binds nothing and can never match a report's
   * observed identity, so it cannot authorize auto-selection.
   */
  identity_digest: string | null;
  /**
   * null when no threshold admitted any accepted choices: the calibrator is
   * unavailable for this domain/identity and grants nothing.
   */
  threshold_logprob: number | null;
  /** Empirical error rate among accepted calibration choices; null when none. */
  calibration_error_rate: number | null;
  calibration_accepted: number;
  calibration_size: number;
  per_class: Record<string, { size: number; accepted: number; wrong: number; error_rate: number | null }>;
  confidence_method: string;
  confidence_method_description: string;
}

interface RowScore {
  score: number | null;
  chosen: string | null;
  state: JudgmentState;
}

async function runRowScore(client: JevClient, domain: DomainDefinition, row: FixtureRow): Promise<RowScore> {
  const candidates = domain.rowCandidates ? domain.rowCandidates(row) : domain.candidates;
  const input: JudgmentInput = {
    // UUID: the core types DecisionId as a UUID, and the id is stable per
    // (domain, row), so a replayed row keeps the same identity.
    decision_id: decisionIdFor(`${domain.domain}:${row.id}`),
    domain: domain.domain,
    domain_version: domain.version,
    criterion: domain.prompt,
    candidates: [...candidates],
    input_refs: [row.id],
    input_digest: row.content_digest,
    route_policy_digest: promptDigest(domain),
  };
  try {
    const record = await client.judge(input, new AbortController().signal);
    if (record.state !== "available" || record.choice === undefined) {
      return { score: null, chosen: null, state: record.state };
    }
    // The fit feature is the RAW first-token log probability only. A calibrated
    // selective-risk score is itself a fitted bound, so using it here would be
    // circular; any other score evidence is unknown, and an unknown score is
    // never replaced by an invented value.
    const score = record.score.type === "raw_token_log_prob" ? record.score.value : null;
    return { score, chosen: record.choice, state: record.state };
  } catch {
    return { score: null, chosen: null, state: "unavailable" };
  }
}

async function main(): Promise<void> {
  // The identity is observed once, through the same probe evaluate.ts uses, so a
  // calibrator and a report can be compared directly. No identity => no binding.
  const observedIdentity = live ? await probeRuntimeIdentity((url, init) => fetch(url, init)) : null;
  if (live && !observedIdentity) {
    stderr.write("WARNING: runtime probe failed; the calibrator binds to no identity\n");
  }
  const observedIdentityDigest = observedIdentity?.digest ?? null;

  for (const domain of domainsToRun) {
    const dataset = await buildDataset(domain);
    const calibration = dataset.splits.calibration;

    // Content is resolved by exact digest from the approved calibration rows
    // only; the client re-checks sha256(content) against input_digest itself.
    const texts = new Map<string, string>(calibration.map((row): [string, string] => [row.content_digest, row.text]));
    const client = new JevClient({
      resolveInput: async (input) => {
        const text = texts.get(input.input_digest);
        if (text === undefined) throw new Error("judgment input is not an approved calibration row");
        return text;
      },
      transport: live ? ((url, init) => fetch(url, init)) : offlineTransport,
      max_in_flight: 1,
    });

    // (score, is_wrong) for the calibration rows the runtime actually scored:
    // abstentions, invalid, unavailable and unqualified decisions are excluded
    // rather than counted as accepted choices.
    const scored: Array<{ score: number; wrong: boolean; class: string }> = [];
    for (const row of calibration) {
      const { score, chosen, state } = await runRowScore(client, domain, row);
      if (state !== "available" || score === null || chosen === null || chosen === "ABSTAIN") continue;
      scored.push({ score, wrong: chosen !== row.expected, class: row.class });
    }

    // Fit the monotone threshold on the calibration split only: the highest score
    // such that the empirical error rate among accepted choices stays <= target.
    const TARGET_ERROR = 0.05;
    const ranked = [...scored].sort((a, b) => b.score - a.score);
    let threshold: number | null = null;
    let accepted = 0;
    let wrong = 0;
    for (const { score, wrong: isWrong } of ranked) {
      accepted++;
      if (isWrong) wrong++;
      if (wrong / accepted <= TARGET_ERROR) threshold = score;
      else break; // conservative: stop at the first accepted set that exceeds target
    }

    // Per-class breakdown over the same scored rows.
    const per_class: CalibratorReceipt["per_class"] = {};
    for (const cls of REQUIRED_CLASSES) {
      const classRows = calibration.filter((row) => row.class === cls);
      const classScored = scored.filter((entry) => entry.class === cls);
      const classWrong = classScored.filter((entry) => entry.wrong).length;
      per_class[cls] = {
        size: classRows.length,
        accepted: classScored.length,
        wrong: classWrong,
        error_rate: classScored.length > 0 ? classWrong / classScored.length : null,
      };
    }

    const receipt: CalibratorReceipt = {
      domain: domain.domain,
      dataset_digest: dataset.dataset_digest,
      identity_digest: observedIdentityDigest,
      threshold_logprob: threshold,
      calibration_error_rate: accepted > 0 ? wrong / accepted : null,
      calibration_accepted: accepted,
      calibration_size: scored.length,
      per_class,
      confidence_method: "first_token_logprob",
      confidence_method_description: "selective abstention threshold fitted on the calibration split; a first-token log probability is a raw feature, not a calibrated whole-label confidence, and this receipt grants no auto-select authority on its own",
    };

    stdout.write(JSON.stringify(receipt) + "\n");
  }
}

main().catch((e) => {
  stderr.write("FATAL: " + e.message + "\n");
  exit(1);
});
