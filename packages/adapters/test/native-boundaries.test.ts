import { describe, it, expect } from 'bun:test';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

import { OmpAdapter } from '../src/harness/omp';
import { CodexAdapter } from '../src/harness/codex';
import { OpenCodeAdapter } from '../src/harness/opencode';
import { ClaudeAdapter } from '../src/harness/claude';
import { HermesAdapter } from '../src/harness/hermes';
import { AlphaCodeAdapter } from '../src/harness/alphacode';
import {
  AlphaNativeClient,
  permissionReply,
  requireObservedStop,
  validateAlphaIdentity,
  type AlphaIdentity,
  type NativePermission,
} from '../src/native/alphacode';
import {
  checkTurnBudget,
  canonical,
  deriveActionSpec,
  policySandbox,
  type NativePermissionHost,
} from '../src/harness/native/host';
import type { HarnessAdapter } from '../src/harness/types';
import type { HarnessKind, OpenSessionInput, SessionRef, TurnInput } from '../../contracts/src/generated';

const baseTemp = join(tmpdir(), 'ironbark-native-boundaries');

/** A host that never silently allows: `prepareAction` is the core's decision. */
const host: NativePermissionHost = {
  async state() { return { policy_version: 1, state_digest: 'd'.repeat(64) }; },
  async prepareAction() { throw new Error('CoreDeniedUnknownPermission'); },
  async verifyAnswer() { return false; },
  async recordNativeEvent() { /* noop */ },
};

function input(harness: HarnessKind, overrides: Partial<OpenSessionInput> = {}): OpenSessionInput {
  return {
    run_id: 'run-1',
    project_id: 'proj-1',
    cwd: '/tmp',
    route: { connection_id: 'local', model_id: 'm', harness },
    context: { digest: 'd', brief: '', constraints: [], excerpts: [], artifacts: [], unresolved_questions: [], originating_sessions: [] },
    policy: {
      version: 1,
      project_id: 'proj-1',
      allowed_connection_ids: ['local'],
      allowed_tools: ['read'],
      read_roots: ['/tmp'],
      write_roots: ['/tmp'],
      network_destinations: [],
      local_only: true,
      max_workers: 1,
      max_requests: 4,
      max_cost_microusd: 0,
      spend_policy: 'free_only',
      allow_unknown_cost: false,
    },
    ...overrides,
  };
}

function turn(session: SessionRef, allowedTools: string[]): TurnInput {
  return {
    request_id: 'req-1',
    message_id: 'msg-1',
    content: [],
    allowed_tools: allowedTools.map(name => ({ name, description: '', input_schema: {} })),
    budget: input(session.harness as HarnessKind).policy,
  };
}

function adapters(): Array<{ name: string; adapter: HarnessAdapter }> {
  return [
    { name: 'omp', adapter: new OmpAdapter(host, { binary: 'omp', stateDir: join(baseTemp, 'omp') }) },
    { name: 'codex', adapter: new CodexAdapter(host, { binary: 'codex', home: join(baseTemp, 'codex') }) },
    { name: 'opencode', adapter: new OpenCodeAdapter(host, { binary: 'opencode', stateDir: join(baseTemp, 'opencode') }) },
    { name: 'claude', adapter: new ClaudeAdapter(host, { binary: 'claude', home: join(baseTemp, 'claude') }) },
    { name: 'hermes', adapter: new HermesAdapter(host, { binary: 'hermes', stateDir: join(baseTemp, 'hermes') }) },
  ];
}

describe('native boundary — capability honesty', () => {
  for (const { name, adapter } of adapters()) {
    it(`${name}: never advertises qualified approval/cancel/resume/children`, async () => {
      const d = await adapter.describe();
      expect(d.harness).toBe(name as HarnessKind);
      expect(d.transport.length).toBeGreaterThan(0);
      // A capability is only qualified by a recorded live smoke; `describe`
      // must report unavailable or unqualified, never qualified.
      for (const state of [d.approval, d.cancel, d.resume, d.child_observability]) {
        expect(['unavailable', 'unqualified']).toContain(state);
        expect(state).not.toBe('qualified');
      }
      // An unavailable binary is reported as unavailable, not silently ignored.
      if (d.version === '') {
        expect(d.approval).toBe('unavailable');
      } else {
        expect(d.approval).toBe('unqualified');
      }
    });
  }

  it('omp: probes the real installed CLI and isolates its own home', async () => {
    const adapter = new OmpAdapter(host, { binary: 'omp', stateDir: join(baseTemp, 'omp-probe') });
    const d = await adapter.describe();
    expect(d.harness).toBe('omp');
    expect(d.transport).toBe('stdio-acp');
    // The host has omp installed (18.8.9); when present the version is real.
    if (d.version !== '') {
      expect(d.version).toMatch(/^\d+\.\d+\.\d+/);
    }
  });

  it('alphacode: an unpatched/absent permission extension is refused, never simulated', async () => {
    // A real native client over an inert channel: no process, no daemon.
    const channel = {
      source: (async function* () { /* never yields */ })(),
      async write() { /* recorded by the client, nothing to speak to */ },
      async observeStop(session: string) { return { session_id: session, cancelled: true, active_tools: 0, children: [] }; },
      async close() { /* nothing to close */ },
    };
    const identity: AlphaIdentity = {
      buildDigest: 'a'.repeat(64),
      effectiveRoute: { connection_id: 'local', model_id: 'm' },
      cwd: '/tmp',
      allowedTools: [],
      profile: 'ironbark-managed',
      socket: '/run/ironbark/alphacode.sock',
    };
    // The AlphaCode host seam is permission-shaped: it is handed the native
    // request and returns the core's decision; it never allows by default.
    const alphaHost = {
      async state() { return { policy_version: 1, state_digest: 'd'.repeat(64) }; },
      async prepareAction() { throw new Error('CoreDeniedUnknownPermission'); },
      async verifyAnswer() { return false; },
      async recordNativeEvent() { /* noop */ },
    };
    const adapter = new AlphaCodeAdapter(new AlphaNativeClient(channel, identity), alphaHost);
    const d = await adapter.describe();
    expect(d.harness).toBe('alphacode');
    expect(d.transport).toBe('owned-stdio-acp');
    expect(d.approval).toBe('unqualified');

    // The pinned permission extension is mandatory: a handshake reporting no
    // active gate is refused outright rather than run unenforced.
    expect(() => validateAlphaIdentity({ permissionsVersion: 1, gateActive: false }, identity)).toThrow(/UnpatchedAlphaCode/);
    expect(() => validateAlphaIdentity({}, identity)).toThrow(/UnpatchedAlphaCode/);
    // A wrong build/route/tool set is an identity mismatch, not a silent reuse.
    expect(() => validateAlphaIdentity(
      { permissionsVersion: 1, gateActive: true, buildDigest: 'b'.repeat(64), effectiveRoute: identity.effectiveRoute, cwd: identity.cwd, allowedTools: identity.allowedTools, profile: identity.profile, socket: identity.socket },
      identity,
    )).toThrow(/SessionIdentityMismatch/);

    // A permission reply is bound to the exact request and action.
    const request: NativePermission = {
      request_id: 'req-1',
      action: { session_id: 's', tool_call_id: 'tc', resolved_name: 'write', canonical_input: {}, cwd: '/tmp', route: identity.effectiveRoute, policy_version: 1, state_digest: 'd'.repeat(64) },
    };
    expect(() => permissionReply(request, { ...request, request_id: 'other', allowed: true })).toThrow(/PermissionIdentityMismatch/);
    expect(permissionReply(request, { ...request, allowed: false }).allowed).toBe(false);

    // A cancel is only a stop receipt when the children are observed stopped.
    expect(() => requireObservedStop({ session_id: 's', cancelled: false, active_tools: 1, children: [{ terminated: false }] }, 's')).toThrow(/NativeChildrenNotObservedStopped/);
    expect(() => requireObservedStop({ session_id: 's', cancelled: true, active_tools: 0, children: [{ terminated: true }] }, 's')).not.toThrow();
  });
});

describe('native boundary — no fabricated sessions or events', () => {
  it('a turn without an open session fails instead of emitting events', async () => {
    const adapter = new OmpAdapter(host, { binary: '/nonexistent/omp', stateDir: join(baseTemp, 'omp-none') });
    const session: SessionRef = {
      harness: 'omp',
      native_id: 'never-opened',
      native_version: '0',
      project_id: 'proj-1',
      cwd: '/tmp',
      resolved_route: { connection_id: 'local', model_id: 'm' },
    };
    const collected: unknown[] = [];
    await expect((async () => {
      for await (const event of adapter.turn(session, turn(session, ['read']))) collected.push(event);
    })()).rejects.toThrow();
    expect(collected).toHaveLength(0);
  });

  it('an unavailable binary is reported, never substituted', async () => {
    const adapter = new OmpAdapter(host, { binary: '/nonexistent/omp', stateDir: join(baseTemp, 'omp-missing') });
    const d = await adapter.describe();
    expect(d.version).toBe('');
    expect(d.approval).toBe('unavailable');
    await expect(adapter.open(input('omp'))).rejects.toThrow();
  });

  it('interrupt without a session fails rather than reporting a false stop', async () => {
    const adapter = new CodexAdapter(host, { binary: 'codex', home: join(baseTemp, 'codex-stop') });
    const session: SessionRef = {
      harness: 'codex',
      native_id: 'never-opened',
      native_version: '0',
      project_id: 'proj-1',
      cwd: '/tmp',
      resolved_route: { connection_id: 'local', model_id: 'm' },
    };
    await expect(adapter.interrupt(session)).rejects.toThrow();
  });
});

describe('native boundary — authority comes from core state, not the model', () => {
  it('a permission action binds the session run, project and policy version', () => {
    const session = input('omp');
    const action = deriveActionSpec(session, 'fs.write', 'file_write', { path: '/tmp/x' });
    expect(action.run_id).toBe(session.run_id);
    expect(action.project_id).toBe(session.project_id);
    expect(action.policy_version).toBe(session.policy.version);
    expect(action.kind).toBe('file_write');
    expect(action.scope).toEqual({ harness: 'omp', cwd: session.cwd });
  });

  it('a turn whose budget differs from the bound session is refused', () => {
    const session = input('omp');
    const drifted: TurnInput = {
      request_id: 'req-2',
      message_id: 'msg-2',
      content: [],
      allowed_tools: [{ name: 'read', description: '', input_schema: {} }],
      budget: { ...session.policy, allowed_tools: ['read', 'shell'] },
    };
    expect(() => checkTurnBudget({ input: session }, drifted)).toThrow(/ManagedTurnPolicyMismatch/);
  });

  it('a turn with no allowed tool is refused (tools are never implied)', () => {
    const session = input('omp');
    const tooless: TurnInput = {
      request_id: 'req-3',
      message_id: 'msg-3',
      content: [],
      allowed_tools: [],
      budget: session.policy,
    };
    expect(() => checkTurnBudget({ input: session }, tooless)).toThrow(/ManagedTurnToolsRequired/);
  });

  it('the native sandbox posture is derived from the project policy', () => {
    const readOnly = policySandbox({ ...input('omp').policy, write_roots: [] });
    expect(readOnly.sandbox).toBe('read-only');
    expect(readOnly.approvalPolicy).toBe('on-request');
    const writable = policySandbox({ ...input('omp').policy, write_roots: ['/tmp'] });
    expect(writable.sandbox).toBe('workspace-write');
  });

  it('a resumed session must match the new session identity or be refused', async () => {
    const adapter = new OpenCodeAdapter(host, { binary: 'opencode', stateDir: join(baseTemp, 'opencode-resume') });
    const mismatched: SessionRef = {
      harness: 'opencode',
      native_id: 'other-project-session',
      native_version: '0',
      project_id: 'another-project',
      cwd: '/elsewhere',
      resolved_route: { connection_id: 'local', model_id: 'm' },
    };
    const resumeInput = input('opencode', { resume: mismatched });
    // Either the resume is refused outright, or the session ref it returns is a
    // new, distinct native identity bound to this project — never the borrowed
    // session of another project.
    try {
      const opened = await adapter.open(resumeInput);
      expect(opened.project_id).toBe(resumeInput.project_id);
      expect(opened.native_id).not.toBe(mismatched.native_id);
      expect(canonical(opened.resolved_route)).toBe(canonical(resumeInput.route));
    } catch (error) {
      expect(String(error)).toMatch(/SessionIdentityMismatch|Unavailable|Exit|not/i);
    }
  });
});
