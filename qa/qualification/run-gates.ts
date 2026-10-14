#!/usr/bin/env bun
/**
 * qa/qualification/run-gates.ts — D5 gate runner.
 *
 * The companion of qa/qualification/release.ts. Where the checker binds a
 * manifest's `release_gates.required` to receipts already on disk, this runner
 * produces those receipts by executing the gates for real and recording what
 * actually happened: the exact command line, start/finish timestamps, the
 * observed exit code and a bounded tail of combined output.
 *
 * A receipt is written per gate into the evidence directory under the checker's
 * own `<gate-slug>.json` naming (both helpers are imported from release.ts, so
 * there is one naming convention, not two). A receipt records `state: "passed"`
 * only when the gate's process was observed to exit 0. A gate whose command or
 * script cannot be executed at all is recorded `failed` with the real error —
 * never `passed`. Nothing goes green by omission.
 *
 * CLI:
 *   bun qa/qualification/run-gates.ts [--manifest packaging/manifest.json]
 *                                     [--evidence-dir DIR] [--only <gate>] [--dry-run]
 *
 *   --manifest PATH   manifest to read gates from (default packaging/manifest.json)
 *   --evidence-dir DIR  receipt directory (default: the single plan dir under
 *                     .superpowers/sdd, resolved by release.ts)
 *   --only GATE       run only the required gate whose string is exactly GATE
 *   --dry-run         print the resolved plan (argv + receipt path per gate)
 *                     and write nothing
 *
 * Exit: 0 every selected gate passed, 1 any gate failed (or the manifest could
 * not be read), 2 usage error — matching release.ts's convention.
 */
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { isAbsolute, relative, resolve } from "node:path";
import {
  defaultEvidenceDir,
  gateSlug,
  receiptPathFor,
  type ReleaseManifest,
} from "./release";

/**
 * Paths this repository's gates are meant to prove. These must be stable
 *  across a single gate run: a build output such as `target/debug/ironbark-core`
 *  is rewritten by the `cargo test --workspace` gate itself, so recording its
 *  digest would block on drift the run caused. Source-level digests (the
 *  generated wire contract and its schema) are what a release is bound to; a
 *  path that does not exist is skipped rather than having a digest invented. */
const PROOF_PATHS = [
  "packages/contracts/src/generated.ts",
  "packages/contracts/schema.json",
] as const;

/** Script extensions treated as "run me with bun" when the gate is a lone path. */
const SCRIPT_EXT = /\.(ts|tsx|js|mjs|cjs|sh|py)$/i;

/** Maximum characters of combined output kept in a receipt. */
const MAX_TAIL = 4000;

interface ArtifactOut {
  path: string;
  sha256: string;
}

interface ReceiptOut {
  gate: string;
  state: "passed" | "failed";
  command: string;
  recorded_at: string;
  started_at: string;
  finished_at: string;
  exit_code: number | null;
  output_tail: string;
  artifacts: ArtifactOut[];
}

function display(root: string, path: string): string {
  const rel = relative(root, path);
  return rel === "" || rel.startsWith("..") || isAbsolute(rel) ? path : rel;
}

function tailOf(text: string): string {
  if (text.length <= MAX_TAIL) return text;
  return `…[${text.length - MAX_TAIL} chars truncated]…\n${text.slice(-MAX_TAIL)}`;
}

/**
 * Resolve a gate string to argv. A gate whose string is a file path is executed
 * as `bun <path>`; anything else is split into argv and executed directly, with
 * no shell involved.
 */
function argvForGate(gate: string, root: string): string[] {
  const token = gate.trim();
  if (!/\s/.test(token) && (SCRIPT_EXT.test(token) || existsSync(resolve(root, token)))) {
    return ["bun", token];
  }
  return token.split(/\s+/);
}

/** Digests for the proof paths that exist, recomputed now (after the run). */
async function artifactsFor(root: string): Promise<ArtifactOut[]> {
  const artifacts: ArtifactOut[] = [];
  for (const rel of PROOF_PATHS) {
    const abs = resolve(root, rel);
    if (!existsSync(abs)) continue;
    try {
      artifacts.push({ path: rel, sha256: createHash("sha256").update(await readFile(abs)).digest("hex") });
    } catch {
      // Unreadable is the same as absent here: no digest is invented.
    }
  }
  return artifacts;
}

/** Execute one gate and write its receipt. `state: "passed"` requires exit 0. */
async function runGate(gate: string, root: string, evidenceDir: string): Promise<{ receipt: ReceiptOut; path: string }> {
  const argv = argvForGate(gate, root);
  const started = new Date();
  let exitCode: number | null = null;
  let output: string;
  try {
    const proc = Bun.spawn(argv, {
      cwd: root,
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
    });
    // Bun pipes are byte streams: read both concurrently so neither can block.
    const stdout = new Response(proc.stdout).text();
    const stderr = new Response(proc.stderr).text();
    exitCode = await proc.exited;
    output = [await stdout, await stderr].filter((text) => text.length > 0).join("\n");
  } catch (error) {
    // Missing executable or unspawnable command: a real failure, never passed.
    exitCode = null;
    output = error instanceof Error ? error.message : String(error);
  }
  const finished = new Date();
  const receipt: ReceiptOut = {
    gate,
    state: exitCode === 0 ? "passed" : "failed",
    command: argv.join(" "),
    recorded_at: finished.toISOString(),
    started_at: started.toISOString(),
    finished_at: finished.toISOString(),
    exit_code: exitCode,
    output_tail: tailOf(output),
    artifacts: await artifactsFor(root),
  };
  const path = receiptPathFor(gate, evidenceDir);
  await mkdir(evidenceDir, { recursive: true });
  await writeFile(path, `${JSON.stringify(receipt, null, 2)}\n`);
  return { receipt, path };
}

function usage(): never {
  console.error(
    "usage: bun qa/qualification/run-gates.ts [--manifest <path>] [--evidence-dir <dir>] [--only <gate>] [--dry-run]",
  );
  process.exit(2);
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  let manifestArg: string | undefined;
  let evidenceArg: string | undefined;
  let only: string | undefined;
  let dryRun = false;
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--dry-run") {
      dryRun = true;
      continue;
    }
    if (flag !== "--manifest" && flag !== "--evidence-dir" && flag !== "--only") usage();
    const next: string | undefined = argv[index + 1];
    if (next === undefined || next.startsWith("--")) usage();
    if (flag === "--manifest") manifestArg = next;
    else if (flag === "--evidence-dir") evidenceArg = next;
    else only = next;
    index += 1; // each value flag consumes its value
  }

  const root = process.cwd();
  const manifestPath = resolve(root, manifestArg ?? "packaging/manifest.json");
  let manifest: ReleaseManifest;
  try {
    manifest = JSON.parse(await readFile(manifestPath, "utf8")) as ReleaseManifest;
  } catch (error) {
    console.error(
      `run-gates: manifest ${display(root, manifestPath)} cannot be read: ${error instanceof Error ? error.message : String(error)}`,
    );
    process.exit(1);
  }

  const required: unknown = manifest.release_gates?.required;
  if (!Array.isArray(required) || required.some((gate) => typeof gate !== "string")) {
    console.error(`run-gates: manifest ${display(root, manifestPath)} declares no release_gates.required string array`);
    process.exit(1);
  }
  const gates = required as string[];
  const selected = only === undefined ? gates : gates.filter((gate) => gate === only);
  if (only !== undefined && selected.length === 0) {
    console.error(`run-gates: --only ${JSON.stringify(only)} does not name a required gate of ${display(root, manifestPath)}`);
    process.exit(2);
  }

  const evidenceDir = evidenceArg ? resolve(root, evidenceArg) : await defaultEvidenceDir(root);

  if (dryRun) {
    const plan = {
      dry_run: true,
      manifest: display(root, manifestPath),
      evidence_dir: display(root, evidenceDir),
      gates: selected.map((gate) => ({
        gate,
        slug: gateSlug(gate),
        command: argvForGate(gate, root).join(" "),
        receipt: display(root, receiptPathFor(gate, evidenceDir)),
      })),
    };
    console.log(JSON.stringify(plan, null, 2));
    process.exit(0);
  }

  const results: { gate: string; state: string; command: string; receipt: string; exit_code: number | null }[] = [];
  for (const gate of selected) {
    const { receipt, path } = await runGate(gate, root, evidenceDir);
    results.push({
      gate: receipt.gate,
      state: receipt.state,
      command: receipt.command,
      receipt: display(root, path),
      exit_code: receipt.exit_code,
    });
  }
  const passed = results.filter((result) => result.state === "passed").length;
  console.log(
    JSON.stringify(
      {
        state: passed === results.length ? "passed" : "failed",
        manifest: display(root, manifestPath),
        evidence_dir: display(root, evidenceDir),
        gates: results,
      },
      null,
      2,
    ),
  );
  console.error(`release gates: ${passed}/${results.length} passed`);
  process.exit(passed === results.length ? 0 : 1);
}

if (import.meta.main) {
  await main();
}
