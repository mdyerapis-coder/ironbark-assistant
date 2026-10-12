import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";

type Identity = { pid: number; start_ticks: number; sandbox_identity: number; live: boolean };
type Proof = { test: string; all_terminated: boolean; owned: Identity[]; observed_count: number };
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function positiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}
// Tiny explicit schema for the native-only smoke artifact, not a DTO assertion.
function parseProof(value: unknown): Proof {
  if (!record(value) || typeof value.test !== "string" || typeof value.all_terminated !== "boolean") throw new Error("Invalid containment proof");
  if (value.test === "core_death_terminates_owned_namespace") {
    if (!positiveInteger(value.observed_count)) throw new Error("Invalid core-death count");
    return { test: value.test, all_terminated: value.all_terminated, owned: [], observed_count: value.observed_count };
  }
  if (!Array.isArray(value.owned)) throw new Error("Missing native process identities");
  const raw: unknown[] = value.owned;
  const owned = raw.map((item): Identity => {
    if (!record(item) || !positiveInteger(item.pid) || !positiveInteger(item.start_ticks) || !positiveInteger(item.sandbox_identity) || typeof item.live !== "boolean") throw new Error("Invalid native process identity");
    return { pid: item.pid, start_ticks: item.start_ticks, sandbox_identity: item.sandbox_identity, live: item.live };
  });
  return { test: value.test, all_terminated: value.all_terminated, owned, observed_count: owned.length };
}

// The integration test harness invokes ProcessSupervisor directly and launches
// the actual fixture through the core PID1 runner. No argument-only fake launch.
const root = resolve(import.meta.dir, "../..");
const evidence = await mkdtemp(join(tmpdir(), "ironbark-containment-evidence-"));
 const names = [
   "symlink_and_nonexistent_tail_escape_is_denied",
   "secret_home_is_not_mounted",
   "shell_cannot_bypass_network_allowlist",
   "cancel_kills_grandchild_and_background_process",
   "runtime_ancestor_rename_and_substitution_are_denied",
   "worker_budget_is_enforced_for_live_namespaces",
   "ptrace_and_process_vm_are_denied",
   "core_death_terminates_owned_namespace",
   "private_fd3_is_consumed_then_absent_from_tool",
 ];
try {
  const child = Bun.spawn(["cargo", "test", "-p", "ironbark-core", "--test", "containment", "--", "--test-threads=1"], {
    cwd: root,
    env: { ...process.env, IRONBARK_CONTAINMENT_EVIDENCE: evidence },
    stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, exit] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  process.stdout.write(stdout); process.stderr.write(stderr);
  if (exit !== 0) throw new Error(`Native containment failed (${exit}); namespaces are not qualified`);
  const summary = /test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out/.exec(stdout);
   if (!summary || Number(summary[1]) !== 10 || summary.slice(2).some(value => Number(value) !== 0)) {
    throw new Error("Incomplete native boundary coverage or skipped tests; qualification blocked");
  }
  const proofs = [];
  for (const name of names) {
    const proof = parseProof(JSON.parse(await readFile(join(evidence, `${name}.json`), "utf8")));
    if (proof.test !== name || proof.all_terminated !== true) throw new Error(`Missing real termination evidence: ${name}`);
    if (name === "core_death_terminates_owned_namespace") {
      if (proof.observed_count < 4) throw new Error("Core-death probe did not own detached descendants");
    } else {
      if (proof.owned.length === 0 || proof.owned.some(p => p.live !== false)) {
        throw new Error(`Missing PID/start-tick/namespace exit evidence: ${name}`);
      }
      if (name === "cancel_kills_grandchild_and_background_process" && proof.owned.length < 4) throw new Error("Parent-only cancellation proof rejected");
    }
    proofs.push(proof);
  }
  console.log(JSON.stringify({ containment_effects: "observed", tests: proofs, native_provider_capability: "unqualified", reason: "No live route or per-tool credential isolation is implied by filesystem/process smoke" }));
} finally {
  await rm(evidence, { recursive: true, force: true });
}
