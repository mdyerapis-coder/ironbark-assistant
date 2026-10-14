#!/usr/bin/env bun
/**
 * qa/qualification/release.test.ts — the release checker's own controls (Task D5).
 *
 * These are checker-integrity tests: no release gate is executed here and no
 * real receipt is produced. Synthetic receipts go into a throwaway plan
 * directory under a temp root, and the assertions are about the checker's
 * verdicts — a required gate with no receipt, a receipt that is not "passed",
 * a stale digest or a stale manifest binding must all block, while a complete
 * synthetic set passes. packaging/manifest.json is only ever read.
 *
 * Run: bun test qa/qualification/release.test.ts
 */
import { describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { gateSlug, releaseCheck, type GateReport, type ReleaseManifest, type ReleaseReport } from "./release";

const repositoryRoot = resolve(import.meta.dir, "..", "..");
const planName = "2026-10-14-release-checker-test";
const REQUIRED = ["cargo test --workspace", "qa/smoke/crash-recovery.ts"];
const VERSION = "0.1.0";
const ARTIFACT = "target/debug/ironbark-cli";

/** A throwaway manifest declaring synthetic gates; never the real manifest. */
const MANIFEST: ReleaseManifest = {
  schema_version: "1.0",
  package_name: "release-checker-fixture",
  version: VERSION,
  components: [{ name: "ironbark-core", type: "binary", path: "bin/ironbark-core" }],
  release_gates: {
    required: REQUIRED,
    owner_approved_and_outstanding: ["owner-approved live provider qualification"],
    artwork_rights: "fixture only",
  },
};

interface Fixture {
  root: string;
  planDir: string;
  manifestPath: string;
  artifact: string;
  artifactDigest: string;
}

async function fixture(): Promise<Fixture> {
  const root = await mkdtemp(join(tmpdir(), "ib-release-"));
  const planDir = join(root, ".superpowers", "sdd", planName);
  await mkdir(planDir, { recursive: true });
  const manifestPath = join(root, "manifest.json");
  await writeFile(manifestPath, `${JSON.stringify(MANIFEST, null, 2)}\n`);
  const artifact = join(root, ARTIFACT);
  await mkdir(dirname(artifact), { recursive: true });
  await writeFile(artifact, "fixture artifact, not a real build\n");
  const artifactDigest = createHash("sha256").update(await readFile(artifact)).digest("hex");
  return { root, planDir, manifestPath, artifact, artifactDigest };
}

/** Writes one synthetic receipt by the documented `<gate-slug>.json` naming. */
async function writeReceipt(plan: Fixture, gate: string, body: Record<string, unknown>): Promise<string> {
  const path = join(plan.planDir, `${gateSlug(gate)}.json`);
  await writeFile(path, `${JSON.stringify({ gate, command: `fixture receipt for ${gate}`, ...body }, null, 2)}\n`);
  return path;
}

/** Every required gate of MANIFEST, all passing. */
async function writeCompleteSet(plan: Fixture): Promise<void> {
  for (const [index, gate] of REQUIRED.entries()) {
    await writeReceipt(plan, gate, {
      state: "passed",
      manifest_version: VERSION,
      artifacts:
        index === 0
          ? [{ path: ARTIFACT, sha256: plan.artifactDigest, component: "ironbark-core" }]
          : [],
    });
  }
}

function gateOf(report: ReleaseReport, gate: string): GateReport {
  const found = report.gates.find((entry) => entry.gate === gate);
  if (!found) throw new Error(`report does not mention gate ${gate}`);
  return found;
}

describe("releaseCheck", () => {
  it("passes only when every required gate has a passing receipt", async () => {
    const plan = await fixture();
    try {
      await writeCompleteSet(plan);
      // No evidenceDir: the single plan dir under .superpowers/sdd is discovered.
      const report = await releaseCheck(MANIFEST, { root: plan.root, manifestPath: plan.manifestPath });
      expect(report.state).toBe("passed");
      expect(report.gates.map((gate) => [gate.gate, gate.outcome])).toEqual([
        [REQUIRED[0], "passed"],
        [REQUIRED[1], "passed"],
      ]);
      expect(gateOf(report, REQUIRED[0]).receipt?.endsWith("cargo-test-workspace.json")).toBe(true);
      expect(gateOf(report, REQUIRED[0]).artifacts).toEqual([
        {
          path: ARTIFACT,
          recorded: plan.artifactDigest,
          actual: plan.artifactDigest,
          component: "ironbark-core",
          match: true,
        },
      ]);
      expect(report.components.length).toBe(1);
      expect(report.components[0]).toMatchObject({
        name: "ironbark-core",
        type: "binary",
        path: "bin/ironbark-core",
        optional: false,
      });
      expect(report.components[0]?.verified_by.length).toBe(1);
      expect(report.outstanding).toEqual(["owner-approved live provider qualification"]);
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("blocks when a required gate has no receipt", async () => {
    const plan = await fixture();
    try {
      await writeReceipt(plan, REQUIRED[0], { state: "passed" });
      const report = await releaseCheck(MANIFEST, {
        root: plan.root,
        evidenceDir: plan.planDir,
        manifestPath: plan.manifestPath,
      });
      expect(report.state).toBe("blocked");
      if (report.state !== "blocked") throw new Error("a missing receipt must block the release");
      const missing = gateOf(report, REQUIRED[1]);
      expect(missing.outcome).toBe("blocked");
      expect(missing.receipt).toBeNull();
      expect(missing.reasons.some((reason) => reason.includes("no receipt"))).toBe(true);
      expect(gateOf(report, REQUIRED[0]).outcome).toBe("passed");
      expect(report.blockers.some((blocker) => blocker.startsWith(`${REQUIRED[1]}: no receipt`))).toBe(true);
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("blocks when a receipt state is not passed, and when a receipt is not JSON", async () => {
    const plan = await fixture();
    try {
      await writeReceipt(plan, REQUIRED[0], { state: "blocked_on_owner_wiring" });
      await writeFile(join(plan.planDir, `${gateSlug(REQUIRED[1])}.json`), "not json at all\n");
      const report = await releaseCheck(MANIFEST, { root: plan.root, evidenceDir: plan.planDir });
      expect(report.state).toBe("blocked");
      expect(gateOf(report, REQUIRED[0]).reasons.join(" ")).toContain('state is "blocked_on_owner_wiring"');
      expect(gateOf(report, REQUIRED[1]).reasons.join(" ")).toContain("unreadable");
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("blocks when a receipt omits the command that produced it, and when it names an undeclared component", async () => {
    const plan = await fixture();
    try {
      await writeReceipt(plan, REQUIRED[0], { state: "passed", command: "" });
      await writeReceipt(plan, REQUIRED[1], {
        state: "passed",
        artifacts: [{ path: ARTIFACT, sha256: plan.artifactDigest, component: "namespace-bridge" }],
      });
      const report = await releaseCheck(MANIFEST, { root: plan.root, evidenceDir: plan.planDir });
      expect(report.state).toBe("blocked");
      expect(gateOf(report, REQUIRED[0]).reasons.join(" ")).toContain("does not record the command");
      expect(gateOf(report, REQUIRED[1]).reasons.join(" ")).toContain("the manifest does not declare");
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("blocks when a recorded artifact digest no longer matches the tree", async () => {
    const plan = await fixture();
    try {
      await writeCompleteSet(plan);
      expect((await releaseCheck(MANIFEST, { root: plan.root, evidenceDir: plan.planDir })).state).toBe("passed");
      await writeFile(plan.artifact, "mutated after the receipt was written\n");
      const report = await releaseCheck(MANIFEST, { root: plan.root, evidenceDir: plan.planDir });
      expect(report.state).toBe("blocked");
      const checked = gateOf(report, REQUIRED[0]).artifacts[0];
      expect(checked?.match).toBe(false);
      expect(checked?.recorded).toBe(plan.artifactDigest);
      expect(checked?.actual).not.toBe(plan.artifactDigest);
      expect(gateOf(report, REQUIRED[0]).reasons.join(" ")).toContain("digest changed");
      expect(gateOf(report, REQUIRED[1]).outcome).toBe("passed");
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("blocks a stale receipt and a throwaway manifest whose required set was edited, then passes with the full set", async () => {
    const canonicalPath = join(repositoryRoot, "packaging", "manifest.json");
    const canonical = JSON.parse(await readFile(canonicalPath, "utf8")) as ReleaseManifest;
    const canonicalGates = canonical.release_gates;
    if (!canonicalGates) throw new Error("packaging/manifest.json declares no release_gates block");
    const declared = canonicalGates.required;
    expect(Array.isArray(declared)).toBe(true);
    const gates = declared as string[];
    expect(gates.length).toBeGreaterThan(1);
    const manifestDigest = createHash("sha256").update(await readFile(canonicalPath)).digest("hex");
    const plan = await fixture();
    try {
      // A stale receipt (different manifest version) blocks even when its state is "passed".
      await writeReceipt(plan, gates[0], { state: "passed", manifest_version: "0.0.9" });
      const stale = await releaseCheck(canonical, {
        root: plan.root,
        evidenceDir: plan.planDir,
        manifestPath: canonicalPath,
      });
      expect(stale.state).toBe("blocked");
      expect(gateOf(stale, gates[0]).reasons.join(" ")).toContain("is stale");

      // A throwaway manifest that drops a required gate cannot go green without evidence.
      await rm(join(plan.planDir, `${gateSlug(gates[0])}.json`));
      const throwaway: ReleaseManifest = {
        ...canonical,
        release_gates: { ...canonicalGates, required: gates.slice(1) },
      };
      const throwawayPath = join(plan.root, "throwaway-manifest.json");
      await writeFile(throwawayPath, `${JSON.stringify(throwaway, null, 2)}\n`);
      const edited = await releaseCheck(throwaway, {
        root: plan.root,
        evidenceDir: plan.planDir,
        manifestPath: throwawayPath,
      });
      expect(edited.state).toBe("blocked");
      expect(edited.gates.map((gate) => gate.gate)).toEqual(gates.slice(1));
      expect(edited.gates.every((gate) => gate.outcome === "blocked")).toBe(true);
      expect(edited.gates.some((gate) => gate.gate === gates[0])).toBe(false);

      // Restoring the gate and giving every real gate its receipt passes, and the
      // manifest digest binding is verified rather than trusted by presence.
      for (const gate of gates) {
        await writeReceipt(plan, gate, {
          state: "passed",
          manifest_version: canonical.version,
          ...(gate === gates[0] ? { manifest_sha256: manifestDigest } : {}),
        });
      }
      const restored = await releaseCheck(canonical, {
        root: plan.root,
        evidenceDir: plan.planDir,
        manifestPath: canonicalPath,
      });
      expect(restored.state).toBe("passed");
      expect(restored.gates.map((gate) => gate.gate)).toEqual(gates);
      expect(gateOf(restored, gates[0]).recorded_state).toBe("passed");
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });
});

/** Runs the real entry point exactly as the documented CLI is invoked. */
async function runCli(args: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
  const child = Bun.spawn([process.execPath, join(repositoryRoot, "qa", "qualification", "release.ts"), ...args], {
    cwd: repositoryRoot,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  // Bun pipes are byte streams: read both concurrently so neither can block the child.
  const stdout = new Response(child.stdout).text();
  const stderr = new Response(child.stderr).text();
  const code = await child.exited;
  return { code, stdout: await stdout, stderr: await stderr };
}

describe("release.ts CLI", () => {
  it("prints a passed report and exits 0 for a complete receipt set", async () => {
    const plan = await fixture();
    try {
      for (const gate of REQUIRED) await writeReceipt(plan, gate, { state: "passed" });
      const run = await runCli(["--manifest", plan.manifestPath, "--evidence-dir", plan.planDir]);
      expect(run.code).toBe(0);
      const report = JSON.parse(run.stdout) as ReleaseReport;
      expect(report.state).toBe("passed");
      expect(report.gates.map((gate) => gate.gate)).toEqual(REQUIRED);
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("prints a blocked report and exits 1 when a required gate has no receipt", async () => {
    const plan = await fixture();
    try {
      await writeReceipt(plan, REQUIRED[0], { state: "passed" });
      const run = await runCli(["--manifest", plan.manifestPath, "--evidence-dir", plan.planDir]);
      expect(run.code).toBe(1);
      const report = JSON.parse(run.stdout) as ReleaseReport;
      expect(report.state).toBe("blocked");
      expect(gateOf(report, REQUIRED[1]).outcome).toBe("blocked");
      expect(gateOf(report, REQUIRED[0]).outcome).toBe("passed");
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });

  it("rejects an unknown flag, a flag with no value and a flag used as a value, without printing a report", async () => {
    const plan = await fixture();
    try {
      const unknown = await runCli(["--manifest", plan.manifestPath, "--evidence-dir", plan.planDir, "--bogus"]);
      expect(unknown.code).toBe(2);
      expect(unknown.stdout.trim()).toBe("");
      expect(unknown.stderr).toContain("usage: bun qa/qualification/release.ts");
      const valueless = await runCli(["--manifest"]);
      expect(valueless.code).toBe(2);
      expect(valueless.stdout.trim()).toBe("");
      expect(valueless.stderr).toContain("usage: bun qa/qualification/release.ts");
      const swapped = await runCli(["--manifest", "--evidence-dir", plan.planDir]);
      expect(swapped.code).toBe(2);
      expect(swapped.stdout.trim()).toBe("");
    } finally {
      await rm(plan.root, { recursive: true, force: true });
    }
  });
});
