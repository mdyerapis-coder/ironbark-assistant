import assert from "node:assert/strict";
import { readdir, readFile, stat } from "node:fs/promises";
import { JevClient } from "../../packages/adapters/src/jev/client";
import { sha256 } from "../../packages/adapters/src/jev/identity";
import type { JudgmentInput, JudgmentRecord, JevTransport } from "../../packages/adapters/src/jev/types";

// Parent alone runs this explicitly after integrated offline verification.
if (!process.argv.includes("--live")) {
  console.error("No model call made. Parent verification: bun qa/smoke/jev-raw.ts --live");
  process.exit(2);
}

async function runtimeProcesses() {
  const found: { pid: number; start_ticks: string; model_path_digest: string; model_stat: string }[] = [];
  for (const pid of await readdir("/proc")) {
    if (!/^\d+$/.test(pid)) continue;
    try {
      const args = (await readFile(`/proc/${pid}/cmdline`, "utf8")).split("\0").filter(Boolean);
      if (!args[0]?.includes("llama-server")) continue;
      const portIndex = args.indexOf("--port");
      if (args[portIndex + 1] !== "8091" && !args.includes("--port=8091")) continue;
      const modelIndex = args.indexOf("-m");
      const model = modelIndex >= 0 ? args[modelIndex + 1] : undefined;
      assert.ok(model, "Cannot observe runtime model argument");
      const modelInfo = await stat(model);
      const processStat = await readFile(`/proc/${pid}/stat`, "utf8");
      const fields = processStat.slice(processStat.lastIndexOf(")") + 2).split(" ");
      found.push({ pid: Number(pid), start_ticks: fields[19], model_path_digest: sha256(model),
        model_stat: `${modelInfo.dev}:${modelInfo.ino}:${modelInfo.size}:${modelInfo.mtimeMs}` });
    } catch (error) {
      // /proc entries may exit while enumerating. An identified runtime with an
      // unreadable model cannot silently count as unchanged.
      if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error;
    }
  }
  assert.equal(found.length, 1, "Must observe exactly one existing raw runtime; do not start or switch one");
  return found;
}

const baseline = await runtimeProcesses();
const samples = [
  { domain: "intent", text: "Synthetic request: implement a small function to sum two numbers.",
    criterion: "Classify the requested activity; abstain if ambiguous.", candidates: ["IMPLEMENT", "REVIEW", "CHAT", "RESEARCH"], expected: "IMPLEMENT" },
  { domain: "intent", text: "Synthetic request: review this small patch for correctness, without editing it.",
    criterion: "Classify the requested activity; abstain if ambiguous.", candidates: ["IMPLEMENT", "REVIEW", "CHAT", "RESEARCH"], expected: "REVIEW" },
  { domain: "intent", text: "Synthetic request: hello, how are you?",
    criterion: "Classify the requested activity; abstain if ambiguous.", candidates: ["IMPLEMENT", "REVIEW", "CHAT", "RESEARCH"], expected: "CHAT" },
  { domain: "intent", text: "Synthetic request: research the difference between two sorting algorithms.",
    criterion: "Classify the requested activity; abstain if ambiguous.", candidates: ["IMPLEMENT", "REVIEW", "CHAT", "RESEARCH"], expected: "RESEARCH" },
  { domain: "relevance", text: "Synthetic query: how to cancel a task. Candidate excerpt: an AbortSignal allows cancellation.",
    criterion: "Is the candidate excerpt relevant to the query? Abstain if ambiguous.", candidates: ["RELEVANT", "IRRELEVANT"], expected: "RELEVANT" },
  { domain: "handoff", text: "Synthetic handoff: source was edited but no test run or verification receipt exists.",
    criterion: "Classify whether additional evidence is needed. Never interpret this as permission or proof of completion.",
    candidates: ["NEEDS_EVIDENCE", "SUFFICIENT_EVIDENCE"], expected: "NEEDS_EVIDENCE" },
];
let content = "";
const live = new JevClient({ resolveInput: async () => content });
const records: { case: string; expected?: string; matches_expected?: boolean; evidence: "live_raw" | "transport_fixture"; record: JudgmentRecord }[] = [];
function judgment(sample: typeof samples[number], index: number): JudgmentInput {
  return { decision_id: `00000000-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
    domain: sample.domain, domain_version: "raw-smoke-v1", candidates: sample.candidates,
    input_refs: [`synthetic:jev-raw:${index}`], input_digest: sha256(sample.text), criterion: sample.criterion,
    route_policy_digest: sha256("synthetic-no-routing-no-permissions") };
}

let blocked = false;
for (const [index, sample] of samples.entries()) {
  content = sample.text;
  const record = await live.judge(judgment(sample, index), new AbortController().signal);
  assert.equal(record.qualification_digest, undefined);
  assert.notEqual(record.score.type, "calibrated_selective_risk");
  if (record.state !== "available") assert.equal(record.choice, undefined);
  if (record.state === "unavailable" || record.state === "unqualified") blocked = true;
  records.push({ case: `${sample.domain}:${index}`, expected: sample.expected,
    matches_expected: record.choice === sample.expected, evidence: "live_raw", record });
}

content = samples[0].text;
// Decoder fixture is independently tested offline; retain real reported model in
// the envelope to exercise exact-code failure rather than model disagreement.
const fixtureTransport: JevTransport = async (url, init) => {
  if (!url.endsWith("/completion")) return fetch(url, init);
  const props = await fetch("http://127.0.0.1:8091/props", { signal: init.signal, redirect: "error" });
  assert.ok(props.ok);
  const metadata = await props.json() as { model_alias: string };
  return Response.json({ content: "C", stop: true, stop_type: "eos", truncated: false, model: metadata.model_alias });
};
const invalid = await new JevClient({ resolveInput: async () => content, transport: fixtureTransport })
  .judge(judgment(samples[0], 20), new AbortController().signal);
if (invalid.state === "unavailable" || invalid.state === "unqualified") blocked = true;
else { assert.equal(invalid.state, "invalid"); assert.equal(invalid.choice, undefined); }
records.push({ case: "invalid-output", evidence: "transport_fixture", record: invalid });

const cancel = new AbortController();
const cancelled = await new JevClient({ resolveInput: async () => content, transport: async () => {
  cancel.abort(); return new Promise<Response>(() => {});
} }).judge(judgment(samples[0], 21), cancel.signal);
assert.equal(cancelled.state, "unavailable"); assert.equal(cancelled.reason, "cancelled");
records.push({ case: "cancel-in-flight", evidence: "transport_fixture", record: cancelled });

const unavailable = await new JevClient({ resolveInput: async () => content,
  transport: async () => { throw new Error("Synthetic outage; no remote fallback"); } })
  .judge(judgment(samples[0], 22), new AbortController().signal);
assert.equal(unavailable.state, "unavailable"); assert.equal(unavailable.choice, undefined);
records.push({ case: "service-unavailable", evidence: "transport_fixture", record: unavailable });

const after = await runtimeProcesses();
assert.deepEqual(after, baseline, "Shared runtime PID/start time/model stat must remain unchanged");
console.log(JSON.stringify({ method: "J1 raw adapter smoke; not J2 domain qualification", records,
  runtime_before: baseline, runtime_after: after, runtime_unchanged: true, blocked,
  historical_facade_smoke: "2026-10-12: 1/4 matches remains unchanged evidence",
  identity_limits: "Lexical metadata plus smoke-only PID/start/model stat; no model-byte attestation or calibrated confidence" }, null, 2));
if (blocked) process.exitCode = 2;
