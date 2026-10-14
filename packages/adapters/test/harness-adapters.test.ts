import { describe, it, expect, beforeAll, afterAll } from 'bun:test';
import { CodexAdapter } from '../src/harness/codex';
import { OpenCodeAdapter } from '../src/harness/opencode';
import { ClaudeAdapter } from '../src/harness/claude';
import { HermesAdapter } from '../src/harness/hermes';
import type { NativePermissionHost } from '../src/harness/native/host';
import type { ActionSpec, SessionRef, HarnessKind, OpenSessionInput } from '../../contracts/src/generated';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const baseTemp = join(tmpdir(), 'ironbark-test');

/** Minimal host for offline tests — records calls but never allows. */
const testHost: NativePermissionHost = {
  async state() { return { policy_version: 0, state_digest: 'test' }; },
  async prepareAction() { throw new Error('testHost.prepareAction not implemented'); },
  async verifyAnswer() { return true; },
  async recordNativeEvent() { /* noop */ },
};

/** Build a default OpenSessionInput for testing. */
function testInput(harness: string, resume?: SessionRef): OpenSessionInput {
  return {
    run_id: `run_${harness}_test`,
    project_id: `proj_${harness}_test`,
    cwd: '/tmp',
    route: { connection_id: 'test', model_id: 'auto', harness: harness as HarnessKind },
    context: { digest: 'd', brief: '', constraints: [], excerpts: [], artifacts: [], unresolved_questions: [], originating_sessions: [] },
    policy: { version: 1, project_id: `proj_${harness}_test`, allowed_connection_ids: [], allowed_tools: [], read_roots: [], write_roots: [], network_destinations: [], local_only: false, max_workers: 1, max_requests: 1, max_cost_microusd: 0, spend_policy: 'free_only', allow_unknown_cost: false },
    resume,
  };
}

function makeCodex(h: NativePermissionHost) { return new CodexAdapter(h, { binary: 'codex', home: join(baseTemp, 'codex') }); }
function makeOpenCode(h: NativePermissionHost) { return new OpenCodeAdapter(h, { binary: 'opencode', stateDir: join(baseTemp, 'opencode') }); }
function makeClaude(h: NativePermissionHost) { return new ClaudeAdapter(h, { binary: 'claude', home: join(baseTemp, 'claude') }); }
function makeHermes(h: NativePermissionHost) { return new HermesAdapter(h, { binary: 'hermes', stateDir: join(baseTemp, 'hermes') }); }

describe('harness adapters — truthful capability discovery', () => {
  describe('codex', () => {
    it('describe_reports_real_version_and_honest_states', async () => {
      const adapter = makeCodex(testHost);
      const d = await adapter.describe();
      expect(d.harness).toBe('codex');
      expect(d.transport).toBe('stdio-json-rpc');
      expect(['unavailable', 'unqualified']).toContain(d.approval);
      expect(['unavailable', 'unqualified']).toContain(d.cancel);
      expect(['unavailable', 'unqualified']).toContain(d.resume);
      expect(['unavailable', 'unqualified']).toContain(d.child_observability);
      if (d.version) {
        expect(d.version.length).toBeGreaterThan(0);
        expect(d.approval).toBe('unqualified');
      } else {
        expect(d.approval).toBe('unavailable');
      }
    });

    it('open_rejects_unavailable_binary', async () => {
      const adapter = new CodexAdapter(testHost, { binary: '/nonexistent/codex', home: join(baseTemp, 'codex-fail') });
      await expect(adapter.open(testInput('codex'))).rejects.toThrow(/CodexUnavailable/);
    });

    it('open_rejects_route_mismatch', async () => {
      const adapter = makeCodex(testHost);
      const d = await adapter.describe();
      if (!d.version) return;
      const input = testInput('codex');
      input.route.harness = 'opencode';
      await expect(adapter.open(input)).rejects.toThrow('SessionIdentityMismatch');
    });

    it('answerPermission_unknown_id_fails_closed', async () => {
      const adapter = makeCodex(testHost);
      await expect(adapter.answerPermission({ native_request_id: 'unknown', action: {} as ActionSpec, decision: 'allow' })).rejects.toThrow('CodexNoOpenSession');
    });
  });

  describe('opencode', () => {
    it('describe_reports_real_version_and_honest_states', async () => {
      const adapter = makeOpenCode(testHost);
      const d = await adapter.describe();
      expect(d.harness).toBe('opencode');
      expect(d.transport).toBe('stdio-acp');
      expect(['unavailable', 'unqualified']).toContain(d.approval);
      expect(['unavailable', 'unqualified']).toContain(d.cancel);
      expect(['unavailable', 'unqualified']).toContain(d.resume);
      expect(['unavailable', 'unqualified']).toContain(d.child_observability);
      if (d.version) {
        expect(d.version.length).toBeGreaterThan(0);
        expect(d.approval).toBe('unqualified');
      } else {
        expect(d.approval).toBe('unavailable');
      }
    });

    it('open_rejects_unavailable_binary', async () => {
      const adapter = new OpenCodeAdapter(testHost, { binary: '/nonexistent/opencode', stateDir: join(baseTemp, 'opencode-fail') });
      await expect(adapter.open(testInput('opencode'))).rejects.toThrow(/opencodeUnavailable/);
    });

    it('open_rejects_route_mismatch', async () => {
      const adapter = makeOpenCode(testHost);
      const d = await adapter.describe();
      if (!d.version) return;
      const input = testInput('opencode');
      input.route.harness = 'codex';
      await expect(adapter.open(input)).rejects.toThrow('SessionIdentityMismatch');
    });

    it('answerPermission_unknown_id_fails_closed', async () => {
      const adapter = makeOpenCode(testHost);
      await expect(adapter.answerPermission({ native_request_id: 'unknown', action: {} as ActionSpec, decision: 'allow' })).rejects.toThrow('AcpNoOpenSession');
    });
  });

  describe('claude', () => {
    it('describe_reports_real_version_and_honest_states', async () => {
      const adapter = makeClaude(testHost);
      const d = await adapter.describe();
      expect(d.harness).toBe('claude');
      expect(d.transport).toBe('stdio-stream-json');
      expect(['unavailable', 'unqualified']).toContain(d.approval);
      expect(['unavailable', 'unqualified']).toContain(d.cancel);
      expect(['unavailable', 'unqualified']).toContain(d.resume);
      expect(['unavailable', 'unqualified']).toContain(d.child_observability);
      if (d.version) {
        expect(d.version.length).toBeGreaterThan(0);
        expect(d.approval).toBe('unqualified');
      } else {
        expect(d.approval).toBe('unavailable');
      }
    });

    it('open_rejects_unavailable_binary', async () => {
      const adapter = new ClaudeAdapter(testHost, { binary: '/nonexistent/claude', home: join(baseTemp, 'claude-fail') });
      await expect(adapter.open(testInput('claude'))).rejects.toThrow(/ClaudeUnavailable/);
    });

    it('open_rejects_route_mismatch', async () => {
      const adapter = makeClaude(testHost);
      const d = await adapter.describe();
      if (!d.version) return;
      const input = testInput('claude');
      input.route.harness = 'codex';
      await expect(adapter.open(input)).rejects.toThrow('SessionIdentityMismatch');
    });

    it('answerPermission_unknown_id_fails_closed', async () => {
      const adapter = makeClaude(testHost);
      await expect(adapter.answerPermission({ native_request_id: 'unknown', action: {} as ActionSpec, decision: 'allow' })).rejects.toThrow('ClaudeNoOpenSession');
    });
  });

  describe('hermes', () => {
    it('describe_reports_real_version_and_honest_states', async () => {
      const adapter = makeHermes(testHost);
      const d = await adapter.describe();
      expect(d.harness).toBe('hermes');
      expect(d.transport).toBe('stdio-acp');
      expect(['unavailable', 'unqualified']).toContain(d.approval);
      expect(['unavailable', 'unqualified']).toContain(d.cancel);
      expect(['unavailable', 'unqualified']).toContain(d.resume);
      expect(['unavailable', 'unqualified']).toContain(d.child_observability);
      if (d.version) {
        expect(d.version.length).toBeGreaterThan(0);
        expect(d.approval).toBe('unqualified');
      } else {
        expect(d.approval).toBe('unavailable');
      }
    });

    it('open_rejects_unavailable_binary', async () => {
      const adapter = new HermesAdapter(testHost, { binary: '/nonexistent/hermes', stateDir: join(baseTemp, 'hermes-fail') });
      await expect(adapter.open(testInput('hermes'))).rejects.toThrow(/hermesUnavailable/);
    });

    it('open_rejects_route_mismatch', async () => {
      const adapter = makeHermes(testHost);
      const d = await adapter.describe();
      if (!d.version) return;
      const input = testInput('hermes');
      input.route.harness = 'codex';
      await expect(adapter.open(input)).rejects.toThrow('SessionIdentityMismatch');
    });

    it('answerPermission_unknown_id_fails_closed', async () => {
      const adapter = makeHermes(testHost);
      await expect(adapter.answerPermission({ native_request_id: 'unknown', action: {} as ActionSpec, decision: 'allow' })).rejects.toThrow('AcpNoOpenSession');
    });
  });
});

// A live round-trip needs an authenticated provider/CLI session, which the plan
// treats as owner-gated live qualification (the same gate the live smokes
// document). Without the explicit opt-in below the provider-less engines
// (claude/hermes on a machine with no configured provider) would fail on
// startup rather than on the adapter's behaviour, so the live round-trip runs
// only when the owner enables it: IRONBARK_LIVE_HARNESS=1.
const LIVE_HARNESS = process.env['IRONBARK_LIVE_HARNESS'] === '1';
if (!LIVE_HARNESS) {
  console.log('skipping live harness round-trip: set IRONBARK_LIVE_HARNESS=1 to run it (owner-gated live qualification)');
}

describe.skipIf(!LIVE_HARNESS)('harness adapters — real offline round-trip (open/exportContext/close)', () => {
  const harnesses = [
    { name: 'codex', factory: makeCodex },
    { name: 'opencode', factory: makeOpenCode },
    { name: 'claude', factory: makeClaude },
    { name: 'hermes', factory: makeHermes },
  ] as const;

  for (const { name, factory } of harnesses) {
    describe(name, () => {
      let adapter: import('../src/harness/types').HarnessAdapter;
      let sessionRef: SessionRef;

      beforeAll(async () => {
        adapter = factory(testHost);
        const d = await adapter.describe();
        if (d.version === '') {
          console.log(`${name}: binary not callable, skipping real round-trip`);
          return;
        }
        sessionRef = await adapter.open(testInput(name));
        expect(sessionRef.harness).toBe(name);
        expect(sessionRef.native_id.length).toBeGreaterThan(0);
      }, 120_000);

      afterAll(async () => {
        if (sessionRef && adapter) {
          await adapter.close(sessionRef).catch(() => undefined);
        }
      });

      if (name === 'hermes') it('hermes session/new is slow (~10s)', () => {});

      it('exportContext_returns_valid_package', async () => {
        if (!sessionRef) return;
        const ctx = await adapter.exportContext(sessionRef);
        expect(ctx.digest).toBeDefined();
        expect(ctx.brief).toBeDefined();
        expect(ctx.originating_sessions).toContainEqual(sessionRef);
      });

      it('answerPermission_unknown_id_fails_closed', async () => {
        if (!sessionRef) return;
        await expect(adapter.answerPermission({ native_request_id: 'unknown', action: {} as ActionSpec, decision: 'allow' })).rejects.toThrow(/UnknownNativePermission|NoOpenSession/);
      });

      it('interrupt_without_active_turn_throws', async () => {
        if (!sessionRef) return;
        await expect(adapter.interrupt(sessionRef)).rejects.toThrow(/NoActiveTurn/);
      });
    });
  }
});