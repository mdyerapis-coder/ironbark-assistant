#!/usr/bin/env bun
/**
 * qa/qualification/release.ts — D5 release gate checker.
 *
 * Binds every gate in a packaging manifest's `release_gates.required` to actual
 * evidence on disk. A release is `passed` only when every required gate has a
 * receipt that says so; a gate with no receipt, an unreadable or malformed
 * receipt, a receipt whose state is not "passed", a stale artifact digest or a
 * stale manifest binding is `blocked`. Nothing here throws for missing
 * evidence, and nothing goes green by omission: a manifest with no required
 * gates is blocked too.
 *
 * A receipt is a JSON file written by the smoke/check that produced it, under
 * the plan's evidence directory (`.superpowers/sdd/<plan>/`, overridable with
 * --evidence-dir), named after the gate it backs:
 *
 *   <evidence-dir>/<gate-slug>.json        e.g. cargo-test-workspace.json
 *
 * A receipt that declares its own `gate` is found by that field too, so an
 * existing smoke that writes e.g. C4-smoke.json is bound by adding
 * `"gate": "qa/smoke/crash-recovery.ts"` rather than renaming the file.
 *
 * Receipt fields read here:
 *   state             required; only "passed" counts.
 *   command           required; the command that produced the receipt.
 *   gate              optional; must name the required gate it backs.
 *   manifest_version  optional; must equal the manifest's version.
 *   manifest_sha256   optional; must equal the sha256 of the manifest file.
 *   artifacts         optional; [{ path, sha256, component? }] — each digest is
 *                     recomputed from the tree and must still match, and a
 *                     named component must be declared in the manifest.
 *
 * CLI: bun qa/qualification/release.ts --manifest packaging/manifest.json [--evidence-dir DIR]
 * stdout is the report as JSON. Exit: 0 passed, 1 blocked, 2 usage error.
 */
import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { isAbsolute, join, relative, resolve } from "node:path";

/** One entry of the manifest's `components` array, as far as the checker reads it. */
export interface ManifestComponent {
  name: string;
  type: string;
  path: string;
  optional: boolean;
  description?: string;
}

/** The parts of packaging/manifest.json the release checker reads. */
export interface ReleaseManifest {
  schema_version?: unknown;
  package_name?: unknown;
  version?: unknown;
  release_gates?: {
    required?: unknown;
    owner_approved_and_outstanding?: unknown;
    note?: unknown;
    artwork_rights?: unknown;
  };
  components?: unknown;
  [key: string]: unknown;
}

/** A `components[]` entry before it is validated: every field is untrusted. */
interface RawComponent {
  name?: unknown;
  type?: unknown;
  path?: unknown;
  optional?: unknown;
  description?: unknown;
}

/** The fields read from an evidence receipt: every field is untrusted. */
interface RawReceipt {
  state?: unknown;
  command?: unknown;
  gate?: unknown;
  manifest_version?: unknown;
  manifest_sha256?: unknown;
  artifacts?: unknown;
}

/** An `artifacts[]` entry before it is validated. */
interface RawArtifact {
  path?: unknown;
  sha256?: unknown;
  component?: unknown;
}

export interface ArtifactCheck {
  path: string;
  recorded: string;
  actual: string | null;
  component: string | null;
  match: boolean;
}

export interface GateReport {
  gate: string;
  outcome: "passed" | "blocked";
  receipt: string | null;
  recorded_state: string | null;
  command: string | null;
  artifacts: ArtifactCheck[];
  reasons: string[];
}

export interface ComponentReport {
  name: string;
  type: string;
  path: string;
  optional: boolean;
  /** Receipts whose digest-checked artifacts named this component. */
  verified_by: string[];
}

export interface PassedReleaseReport {
  state: "passed";
  manifest: string | null;
  version: string | null;
  components: ComponentReport[];
  gates: GateReport[];
  /** Gates the owner still owes, from `release_gates.owner_approved_and_outstanding`. */
  outstanding: string[];
}

export interface BlockedReleaseReport {
  state: "blocked";
  manifest: string | null;
  version: string | null;
  components: ComponentReport[];
  gates: GateReport[];
  outstanding: string[];
  blockers: string[];
}

export type ReleaseReport = PassedReleaseReport | BlockedReleaseReport;

export interface ReleaseCheckOptions {
  /** Repository root: artifact paths and the default evidence dir resolve here. Default: process.cwd(). */
  root?: string;
  /** Receipt directory, absolute or relative to root. Default: the single plan dir under .superpowers/sdd. */
  evidenceDir?: string;
  /** Manifest file on disk, for manifest_sha256 binding and for the report. */
  manifestPath?: string;
}

const HEX64 = /^[0-9a-f]{64}$/;

const sha256 = (bytes: Uint8Array): string => createHash("sha256").update(bytes).digest("hex");

/** Receipt-file stem for a gate: lowercase, every run of non-alphanumerics becomes one dash. */
export function gateSlug(gate: string): string {
  const slug = gate.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  return slug.slice(0, 120) || "gate";
}

/** Receipt file a gate must have, by the naming convention above. */
export function receiptPathFor(gate: string, evidenceDir: string): string {
  return join(evidenceDir, `${gateSlug(gate)}.json`);
}

/** The evidence directory for a repository root: its single sdd plan dir, else the sdd root. */
export async function defaultEvidenceDir(root: string): Promise<string> {
  const base = join(root, ".superpowers", "sdd");
  try {
    const dirs = (await readdir(base, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
    if (dirs.length === 1) return join(base, dirs[0]);
  } catch {
    // No evidence has been produced yet: the base path is reported as unreadable.
  }
  return base;
}

export async function releaseCheck(
  manifest: ReleaseManifest,
  options: ReleaseCheckOptions = {},
): Promise<ReleaseReport> {
  const root = resolve(options.root ?? process.cwd());
  const manifestPath = options.manifestPath ? resolve(root, options.manifestPath) : null;
  try {
    const facts = readManifest(manifest);
    let manifestDigest: string | null = null;
    if (manifestPath) {
      try {
        manifestDigest = sha256(await readFile(manifestPath));
      } catch (error) {
        facts.blockers.push(`manifest ${display(root, manifestPath)} cannot be read: ${message(error)}`);
      }
    }
    const evidenceDir = options.evidenceDir
      ? resolve(root, options.evidenceDir)
      : await defaultEvidenceDir(root);
    const evidence = await readEvidenceDir(evidenceDir, root);
    const gates: GateReport[] = [];
    for (const gate of facts.required) {
      gates.push(
        await checkGate(gate, {
          root,
          evidenceDir,
          receipts: evidence.receipts,
          components: facts.components,
          version: facts.version,
          manifestDigest,
        }),
      );
    }
    const components: ComponentReport[] = facts.components.map((component) => ({
      name: component.name,
      type: component.type,
      path: component.path,
      optional: component.optional,
      verified_by: [
        ...new Set(
          gates.flatMap((gate) =>
            gate.artifacts.some((artifact) => artifact.component === component.name && artifact.match)
              ? [gate.receipt ?? gate.gate]
              : [],
          ),
        ),
      ],
    }));
    const blockers = [
      ...facts.blockers,
      ...(evidence.problem ? [evidence.problem] : []),
      ...gates.flatMap((gate) => gate.reasons.map((reason) => `${gate.gate}: ${reason}`)),
    ];
    const report = {
      manifest: manifestPath ? display(root, manifestPath) : null,
      version: facts.version,
      components,
      gates,
      outstanding: facts.outstanding,
    };
    if (blockers.length === 0) return { state: "passed", ...report };
    return { state: "blocked", ...report, blockers };
  } catch (error) {
    // Missing or damaged evidence is Blocked, never an exception.
    return {
      state: "blocked",
      manifest: manifestPath ? display(root, manifestPath) : null,
      version: null,
      components: [],
      gates: [],
      outstanding: [],
      blockers: [`release check failed: ${message(error)}`],
    };
  }
}

interface ManifestFacts {
  required: string[];
  outstanding: string[];
  components: ManifestComponent[];
  version: string | null;
  blockers: string[];
}

function readManifest(manifest: ReleaseManifest): ManifestFacts {
  const facts: ManifestFacts = { required: [], outstanding: [], components: [], version: null, blockers: [] };
  const blockers = facts.blockers;
  if (typeof manifest !== "object" || manifest === null || Array.isArray(manifest)) {
    blockers.push("manifest is not a JSON object");
    return facts;
  }
  if (typeof manifest.version === "string") facts.version = manifest.version;

  const declared: unknown = manifest.release_gates;
  if (typeof declared !== "object" || declared === null || Array.isArray(declared)) {
    blockers.push("manifest has no release_gates block");
  } else {
    const gates = declared as { required?: unknown; owner_approved_and_outstanding?: unknown };
    if (Array.isArray(gates.required)) {
      for (const [index, entry] of gates.required.entries()) {
        if (typeof entry === "string" && entry.trim() !== "") facts.required.push(entry);
        else blockers.push(`manifest release_gates.required[${index}] is not a non-empty string`);
      }
      if (facts.required.length === 0) {
        blockers.push(
          "manifest release_gates.required lists no gates; a release with no required gate cannot be reported as passed",
        );
      }
    } else if (gates.required === undefined) {
      blockers.push("manifest release_gates has no required list");
    } else {
      blockers.push("manifest release_gates.required is not an array");
    }
    if (Array.isArray(gates.owner_approved_and_outstanding)) {
      for (const entry of gates.owner_approved_and_outstanding) {
        if (typeof entry === "string" && entry.trim() !== "") facts.outstanding.push(entry);
      }
    }
  }

  if (manifest.components === undefined) {
    blockers.push("manifest declares no components");
  } else if (!Array.isArray(manifest.components)) {
    blockers.push("manifest components is not an array");
  } else {
    const names = new Set<string>();
    for (const [index, value] of (manifest.components as unknown[]).entries()) {
      if (typeof value !== "object" || value === null || Array.isArray(value)) {
        blockers.push(`manifest components[${index}] is not an object`);
        continue;
      }
      const entry = value as RawComponent;
      if (typeof entry.name !== "string" || entry.name.trim() === "") {
        blockers.push(`manifest components[${index}] has no name`);
        continue;
      }
      const name = entry.name;
      const type = typeof entry.type === "string" && entry.type.trim() !== "" ? entry.type : null;
      const path = typeof entry.path === "string" && entry.path.trim() !== "" ? entry.path : null;
      if (type === null) blockers.push(`manifest components[${index}] (${name}) has no type`);
      if (path === null) blockers.push(`manifest components[${index}] (${name}) has no path`);
      if (names.has(name)) blockers.push(`manifest declares component ${name} twice`);
      names.add(name);
      facts.components.push({
        name,
        type: type ?? "",
        path: path ?? "",
        optional: entry.optional === true,
        description: typeof entry.description === "string" ? entry.description : undefined,
      });
    }
  }
  return facts;
}

interface Receipt {
  path: string;
  relative: string;
  json: RawReceipt | null;
  problem: string | null;
}

interface GateContext {
  root: string;
  evidenceDir: string;
  receipts: Receipt[];
  components: ManifestComponent[];
  version: string | null;
  manifestDigest: string | null;
}

async function readEvidenceDir(
  dir: string,
  root: string,
): Promise<{ receipts: Receipt[]; problem: string | null }> {
  try {
    const receipts: Receipt[] = [];
    for (const entry of (await readdir(dir, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
      if (!entry.isFile() || !entry.name.endsWith(".json")) continue;
      const path = join(dir, entry.name);
      const relative = display(root, path);
      try {
        const parsed: unknown = JSON.parse(await readFile(path, "utf8"));
        receipts.push(
          typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
            ? { path, relative, json: parsed as RawReceipt, problem: null }
            : { path, relative, json: null, problem: `${relative} is not a JSON object` },
        );
      } catch (error) {
        receipts.push({ path, relative, json: null, problem: `${relative} is unreadable: ${message(error)}` });
      }
    }
    return { receipts, problem: null };
  } catch (error) {
    return { receipts: [], problem: `evidence directory ${display(root, dir)} is not readable: ${message(error)}` };
  }
}

function matchReceipt(gate: string, context: GateContext): { receipt: Receipt | null; problem: string | null } {
  const named = context.receipts.find((receipt) => receipt.path === receiptPathFor(gate, context.evidenceDir));
  if (named) return { receipt: named, problem: null };
  const declared = context.receipts.find((receipt) => receipt.json?.gate === gate);
  if (declared) return { receipt: declared, problem: null };
  const expected = display(context.root, receiptPathFor(gate, context.evidenceDir));
  return {
    receipt: null,
    problem: `no receipt: neither ${expected} nor a receipt declaring "gate": ${JSON.stringify(gate)} exists`,
  };
}

async function checkGate(gate: string, context: GateContext): Promise<GateReport> {
  const artifacts: ArtifactCheck[] = [];
  const match = matchReceipt(gate, context);
  const missing: GateReport = {
    gate,
    outcome: "blocked",
    receipt: null,
    recorded_state: null,
    command: null,
    artifacts,
    reasons: [match.problem ?? "no receipt"],
  };
  if (!match.receipt) return missing;
  const receipt = match.receipt;
  const relative = receipt.relative;
  if (receipt.json === null) {
    return { ...missing, receipt: relative, reasons: [receipt.problem ?? `${relative} is not a JSON object`] };
  }
  const json = receipt.json;
  const reasons: string[] = [];
  const state = typeof json.state === "string" ? json.state : null;
  if (state === null) reasons.push(`receipt ${relative} records no state`);
  else if (state !== "passed") reasons.push(`receipt ${relative} state is "${state}", not "passed"`);
  const command = typeof json.command === "string" && json.command.trim() !== "" ? json.command : null;
  if (command === null) reasons.push(`receipt ${relative} does not record the command that produced it`);

  if (json.gate !== undefined && json.gate !== gate) {
    reasons.push(
      `receipt ${relative} declares gate ${JSON.stringify(json.gate)} but is matched for ${JSON.stringify(gate)}`,
    );
  }
  if (json.manifest_version !== undefined && json.manifest_version !== context.version) {
    reasons.push(
      `receipt ${relative} is stale: it records manifest_version ${JSON.stringify(json.manifest_version)} but the manifest is ${JSON.stringify(context.version)}`,
    );
  }
  if (json.manifest_sha256 !== undefined) {
    const recorded = typeof json.manifest_sha256 === "string" ? json.manifest_sha256.toLowerCase() : null;
    if (context.manifestDigest === null) {
      reasons.push(`receipt ${relative} binds the manifest digest but no manifest file was provided`);
    } else if (recorded !== context.manifestDigest) {
      reasons.push(`receipt ${relative} is stale: its manifest digest does not match the manifest file`);
    }
  }
  if (json.artifacts !== undefined) {
    if (!Array.isArray(json.artifacts)) {
      reasons.push(`receipt ${relative} artifacts is not an array`);
    } else {
      for (const [index, value] of (json.artifacts as unknown[]).entries()) {
        const checked = await checkArtifact(value, index, relative, context);
        if (checked.report) artifacts.push(checked.report);
        reasons.push(...checked.reasons);
      }
    }
  }
  return {
    gate,
    outcome: reasons.length === 0 ? "passed" : "blocked",
    receipt: relative,
    recorded_state: state,
    command,
    artifacts,
    reasons,
  };
}

async function checkArtifact(
  value: unknown,
  index: number,
  receiptRelative: string,
  context: GateContext,
): Promise<{ report: ArtifactCheck | null; reasons: string[] }> {
  const label = `receipt ${receiptRelative} artifacts[${index}]`;
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return { report: null, reasons: [`${label} is not an object`] };
  }
  const entry = value as RawArtifact;
  const path = typeof entry.path === "string" && entry.path.trim() !== "" ? entry.path : null;
  if (path === null) return { report: null, reasons: [`${label} has no path`] };
  const recorded =
    typeof entry.sha256 === "string" && HEX64.test(entry.sha256.toLowerCase()) ? entry.sha256.toLowerCase() : null;
  if (recorded === null) return { report: null, reasons: [`${label} (${path}) has no sha256 digest`] };

  const reasons: string[] = [];
  let component: string | null = null;
  if (entry.component !== undefined) {
    const componentName = entry.component;
    const declared = context.components.some((c) => c.name === componentName);
    if (typeof componentName === "string" && declared) component = componentName;
    else {
      reasons.push(
        `${label} (${path}) names component ${JSON.stringify(componentName)}, which the manifest does not declare`,
      );
    }
  }
  const artifactCheck = (actual: string | null): ArtifactCheck => ({
    path,
    recorded,
    actual,
    component,
    match: actual === recorded,
  });

  if (isAbsolute(path)) {
    reasons.push(`${label} (${path}) is absolute; recorded digests must be repository-relative`);
    return { report: artifactCheck(null), reasons };
  }
  const inside = relative(context.root, resolve(context.root, path));
  if (inside === "" || inside.startsWith("..") || isAbsolute(inside)) {
    reasons.push(`${label} (${path}) is outside the repository root`);
    return { report: artifactCheck(null), reasons };
  }
  let actual: string | null = null;
  try {
    actual = sha256(await readFile(resolve(context.root, path)));
  } catch (error) {
    reasons.push(`${label} (${path}) cannot be read: ${message(error)}`);
  }
  if (actual !== null && actual !== recorded) {
    reasons.push(`${label} (${path}) digest changed: the receipt records ${recorded}, the tree has ${actual}`);
  }
  return { report: artifactCheck(actual), reasons };
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function display(root: string, path: string): string {
  const rel = relative(root, path);
  return rel === "" || rel.startsWith("..") || isAbsolute(rel) ? path : rel;
}

function usage(): never {
  console.error("usage: bun qa/qualification/release.ts --manifest <path> [--evidence-dir <dir>]");
  process.exit(2);
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  let manifestArg: string | undefined;
  let evidenceArg: string | undefined;
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag !== "--manifest" && flag !== "--evidence-dir") usage();
    const next: string | undefined = argv[index + 1];
    if (next === undefined || next.startsWith("--")) usage();
    if (flag === "--manifest") manifestArg = next;
    else evidenceArg = next;
    index += 1; // each flag consumes its value
  }
  if (manifestArg === undefined) usage();

  let parsed: unknown;
  try {
    parsed = JSON.parse(await readFile(resolve(manifestArg), "utf8"));
  } catch (error) {
    const report: BlockedReleaseReport = {
      state: "blocked",
      manifest: manifestArg,
      version: null,
      components: [],
      gates: [],
      outstanding: [],
      blockers: [`manifest ${manifestArg} cannot be read: ${message(error)}`],
    };
    console.log(JSON.stringify(report, null, 2));
    process.exit(1);
  }
  const report = await releaseCheck(parsed as ReleaseManifest, {
    root: process.cwd(),
    manifestPath: manifestArg,
    evidenceDir: evidenceArg,
  });
  console.log(JSON.stringify(report, null, 2));
  console.error(
    `release gates: ${report.gates.filter((gate) => gate.outcome === "passed").length}/${report.gates.length} passed — ${report.state}`,
  );
  process.exit(report.state === "passed" ? 0 : 1);
}
