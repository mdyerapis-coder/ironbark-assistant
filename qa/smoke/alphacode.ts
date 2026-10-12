import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import type { AlphaCodeAdapter } from '../../packages/adapters/src/harness/alphacode';
import type { OpenSessionInput, PermissionAnswer, SessionRef, TurnInput, WorkerEvent } from '../../packages/contracts/src/generated';

/** Driver must use real C3-owned processes, Rust policy decisions and independent
 * outbound/descendant observation. This module never fabricates or launches them. */
export type AlphaSmokeDriver = {
  input: OpenSessionInput;
  protected_file: string;
  artifact_file: string;
  artifact_contents: string;
  turns: Record<'denied' | 'stale' | 'edit' | 'failure' | 'replacement' | 'cancel', TurnInput>;
  open(): Promise<AlphaCodeAdapter>;
  decide(event: WorkerEvent, phase: string): Promise<PermissionAnswer>;
  stopWhileRunning(adapter: AlphaCodeAdapter, session: SessionRef, phase: string): Promise<void>;
  observe(): Promise<{ active_processes: number; denied_outbound_attempts: string[]; outbound_destinations: string[]; allowed_destinations: string[] }>;
};
export async function runAlphaSmoke(driver: AlphaSmokeDriver) {
  assert.equal(driver.input.route.harness, 'alphacode');
  assert.ok(driver.input.route.connection_id && driver.input.route.model_id && driver.input.route.model_id !== 'auto');
  const before = await readFile(driver.protected_file);
  const evidence: Record<string, WorkerEvent[]> = {};
  let adapter = await driver.open();
  let session = await adapter.open(driver.input);
  const originalId = session.native_id;
  const collect = async (phase: keyof AlphaSmokeDriver['turns']) => {
    const events: WorkerEvent[] = []; let interrupted = false;
    for await (const event of adapter.turn(session, driver.turns[phase])) {
      events.push(event);
      if (event.payload.type === 'approval_needed') {
        const answer = await driver.decide(event, phase);
        if (phase === 'denied') assert.equal(answer.decision, 'deny');
        if (phase === 'stale') {
          assert.equal(answer.decision, 'allow', 'Supply a genuine old grant for Rust to reject');
          assert.ok(answer.grant, 'Stale-grant scenario cannot substitute an ordinary denial');
          assert.ok(answer.grant.policy_version !== answer.action.policy_version || answer.grant.expected_state_digest !== answer.action.expected_state_digest || answer.grant.revoked_at !== undefined, 'Require actual policy/state staleness or revocation');
        }
        await adapter.answerPermission(answer);
      }
      if (phase === 'cancel' && !interrupted && event.payload.type === 'tool_started') { interrupted = true; await driver.stopWhileRunning(adapter, session, phase); }
    }
    assert.ok(events.some(e => e.payload.type === 'turn_started'));
    if (phase === 'denied' || phase === 'stale') {
      assert.ok(events.some(e => e.payload.type === 'approval_needed'), 'Require actual permission request');
      assert.ok(!events.some(e => e.payload.type === 'tool_started'), 'Denied native sink must never start');
    }
    if (phase === 'cancel') assert.ok(interrupted, 'Require cancellation during actual authorized native tool execution');
    evidence[phase] = events; return events;
  };
  try {
    await collect('denied'); assert.deepEqual(await readFile(driver.protected_file), before);
    await collect('stale'); assert.deepEqual(await readFile(driver.protected_file), before);
    const edits = await collect('edit'); assert.equal(await readFile(driver.artifact_file, 'utf8'), driver.artifact_contents);
    assert.ok(edits.some(e => e.payload.type === 'tool_finished'));
    const failed = await collect('failure'); assert.ok(failed.some(e => e.payload.type === 'tool_finished' && typeof e.payload.result === 'object' && e.payload.result !== null && 'status' in e.payload.result && e.payload.result.status === 'failed'));
    const replacement = await collect('replacement');
    let finalText = '', nativeReplacementText: string | undefined;
    for (const event of replacement) {
      if (event.payload.type === 'text_delta') finalText += event.payload.delta;
      if (event.payload.type === 'text_replace') { nativeReplacementText = event.payload.text; finalText = event.payload.text; }
    }
    assert.notEqual(nativeReplacementText, undefined, 'Live run must actually emit TextReplace');
    assert.equal(finalText, nativeReplacementText);
    const exported = await adapter.exportContext(session); assert.equal(exported.originating_sessions[0]?.native_id, session.native_id);
    await adapter.close(session);
    adapter = await driver.open();
    session = await adapter.open({ ...driver.input, resume: session }); assert.equal(session.native_id, originalId);
    const cancellation = await collect('cancel'); assert.ok(cancellation.some(e => e.payload.type === 'turn_finished' && e.payload.status === 'cancelled'));
  } finally { await adapter.close(session); }
  const observation = await driver.observe();
  assert.equal(observation.active_processes, 0);
  assert.ok(observation.denied_outbound_attempts.length > 0, 'Require actual controlled outbound-deny capture');
  assert.ok(observation.outbound_destinations.every(destination => observation.allowed_destinations.includes(destination)));
  assert.ok(observation.outbound_destinations.every(destination => !/telemetry|github|update|free/i.test(destination)));
  return { qualified: true, native_session_id: originalId, protected_sha256: createHash('sha256').update(before).digest('hex'), evidence, observation };
}
if (import.meta.main) {
  const driverModule = Bun.argv[2]; const outputPath = Bun.argv[3];
  if (!driverModule || !outputPath) throw new Error('Usage: bun qa/smoke/alphacode.ts /absolute/approved-C3-driver.ts /absolute/evidence.json (requires explicit live-route approval)');
  // Runtime-approved driver selection is deliberate; no synthetic default driver.
  const module = await import(driverModule) as { driver: AlphaSmokeDriver };
  await Bun.write(outputPath, JSON.stringify(await runAlphaSmoke(module.driver), null, 2) + '\n');
}
