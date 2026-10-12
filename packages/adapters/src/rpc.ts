import { createReadStream } from 'node:fs';
import type { ConnectionConfig, WorkerProviderTurnInput, ProviderAdapter, WorkerEvent } from '../../contracts/src/generated.ts';
import type { CredentialResolver } from './providers/omp-sdk.ts';

export const MAX_FRAME_BYTES = 8 * 1024 * 1024;
export function encodeFrame(value: unknown, limit = MAX_FRAME_BYTES) {
  const bytes = new TextEncoder().encode(JSON.stringify(value) + '\n');
  if (bytes.length > limit) throw new Error('rpc_frame_oversize');
  return bytes;
}
export async function* decodeFrames(source: AsyncIterable<Uint8Array>, limit = MAX_FRAME_BYTES): AsyncIterable<unknown> {
  let parts: Uint8Array[] = [];
  let size = 0;
  const decoder = new TextDecoder('utf-8', { fatal: true });
  for await (const chunk of source) {
    let start = 0;
    for (let index = 0; index < chunk.length; index++) {
      if (chunk[index] !== 10) continue;
      const part = chunk.subarray(start, index);
      size += part.length;
      if (size + 1 > limit) throw new Error('rpc_frame_oversize');
      parts.push(part);
      const frame = new Uint8Array(size);
      let offset = 0;
      for (const bytes of parts) { frame.set(bytes, offset); offset += bytes.length; }
      parts = []; size = 0; start = index + 1;
      if (!frame.length) throw new Error('rpc_empty_frame');
      yield JSON.parse(decoder.decode(frame));
    }
    if (start < chunk.length) { const part = chunk.slice(start); size += part.length; parts.push(part); }
    if (size + 1 > limit) throw new Error('rpc_frame_oversize');
  }
  if (size) throw new Error('rpc_truncated_frame');
}

// fd 3 is a private native-owned pipe, not JSON/stdout/stderr. Native emits one
// frame per granted lease: uint32 BE lease UTF8 length, uint32 BE secret length,
// lease UTF8 bytes, secret bytes. Limits: 128 and 8192. No prefetch or storage.
export type NativeCredentialResolver = CredentialResolver & { close(): void };
export function nativeCredentialResolver(fd = 3): NativeCredentialResolver {
  const channel = createReadStream('', { fd, autoClose: true, highWaterMark: 8192 });
  const iterator = channel[Symbol.asyncIterator]();
  let buffered = new Uint8Array(0);
  let busy = false;
  let closed = false;
  const consumedLeases = new Set<string>();
  const resolve: CredentialResolver = async (leaseId, signal) => {
    if (busy || closed || consumedLeases.has(leaseId) || consumedLeases.size >= 1024 || !leaseId || new TextEncoder().encode(leaseId).length > 128) throw new Error('credential_lease_denied');
    consumedLeases.add(leaseId);
    busy = true;
    const abort = () => { closed = true; channel.destroy(new Error('credential_cancelled')); };
    signal.addEventListener('abort', abort, { once: true });
    const take = async (length: number) => {
      while (buffered.length < length) {
        signal.throwIfAborted();
        const next = await iterator.next();
        if (next.done) throw new Error('credential_channel_closed');
        const value: Uint8Array = next.value;
        if (buffered.length + value.length > 16384) throw new Error('credential_frame_oversize');
        const merged = new Uint8Array(buffered.length + value.length);
        merged.set(buffered); merged.set(value, buffered.length);
        buffered.fill(0); value.fill(0); buffered = merged;
      }
      const value = buffered.slice(0, length);
      const remaining = buffered.slice(length);
      buffered.fill(0); buffered = remaining;
      return value;
    };
    try {
      signal.throwIfAborted();
      const header = await take(8);
      const view = new DataView(header.buffer, header.byteOffset, header.byteLength);
      const idLength = view.getUint32(0), secretLength = view.getUint32(4);
      header.fill(0);
      if (!idLength || idLength > 128 || secretLength > 8192) throw new Error('credential_frame_invalid');
      const idBytes = await take(idLength);
      const received = new TextDecoder('utf-8', { fatal: true }).decode(idBytes); idBytes.fill(0);
      if (received !== leaseId) throw new Error('credential_lease_mismatch');
      const secret = await take(secretLength);
      if (signal.aborted) { secret.fill(0); signal.throwIfAborted(); }
      return secret;
    } catch {
      closed = true; buffered.fill(0); channel.destroy();
      throw new Error('credential_lease_unavailable');
    } finally { busy = false; signal.removeEventListener('abort', abort); }
  };
  return Object.assign(resolve, { close() { closed = true; buffered.fill(0); channel.destroy(); } });
}

export type AdapterFactory = (connection: ConnectionConfig) => ProviderAdapter;
export type FrameWriter = (frame: Uint8Array) => Promise<void>;
export async function serveProviderRpc(source: AsyncIterable<Uint8Array>, write: FrameWriter, factory: AdapterFactory) {
  let active: { requestId: string; controller: AbortController; task: Promise<void> } | undefined;
  let writer: Promise<void> = Promise.resolve();
  let writeError: unknown;
  // Serialized writer chain: every send is awaited by its caller, so at most
  // two encoded frames (each capped by MAX_FRAME_BYTES) are ever in flight.
  // After the first write failure the chain fails closed; it never retries.
  const send = (frame: unknown) => {
    const bytes = encodeFrame(frame);
    writer = writer.then(() => write(bytes).catch((error) => { writeError = error; throw error; }), () => { throw writeError; });
    return writer;
  };
  let hello = false;
  try {
    for await (const raw of decodeFrames(source)) {
      if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) throw new Error('invalid_rpc');
      const frame = raw as Record<string, unknown>;
      if (frame.protocol !== 1 || typeof frame.request_id !== 'string' || frame.request_id.length > 128) throw new Error('invalid_rpc');
      const requestId = frame.request_id;
      if (frame.method === 'hello' && !hello) {
        hello = true; await send({ protocol: 1, request_id: requestId, type: 'reply', result: { worker: 'provider', sdk_version: '18.8.9' } }); continue;
      }
      if (!hello) throw new Error('rpc_hello_required');
      if (frame.method === 'cancel') {
        const matched = active !== undefined && active.requestId === frame.target_request_id;
        if (matched && active !== undefined) active.controller.abort();
        await send({ protocol: 1, request_id: requestId, type: 'reply', result: { cancelled: matched } }); continue;
      }
      // Read `active` through a local so the `continue` below narrows `busy` (not `active`);
      // otherwise TS narrows `active` to `undefined` at the task closure's definition and the
      // closure's `active?.requestId` check collapses to `never`.
      const busy = active !== undefined;
      if (busy) { await send({ protocol: 1, request_id: requestId, type: 'error', code: 'worker_busy' }); continue; }
      if (frame.method === 'describe') {
        const connection = frame.connection as ConnectionConfig;
        try { await send({ protocol: 1, request_id: requestId, type: 'reply', result: await factory(connection).describe(connection) }); }
        catch { await send({ protocol: 1, request_id: requestId, type: 'error', code: 'invalid_connection' }); }
        continue;
      }
      if (frame.method !== 'stream') { await send({ protocol: 1, request_id: requestId, type: 'error', code: 'unsupported_operation' }); continue; }
      const input = frame.input as WorkerProviderTurnInput;
      if (!input || input.request_id !== requestId || typeof input.run_id !== 'string' || typeof input.message_id !== 'string'
        || !input.connection || !input.route || input.connection.id !== input.route.connection_id || typeof input.credential_lease_id !== 'string') {
        await send({ protocol: 1, request_id: requestId, type: 'error', code: 'invalid_turn' }); continue;
      }
      const controller = new AbortController();
      const task = (async () => {
        try {
          const adapter = factory(input.connection);
          let status = 'failed';
          for await (const event of adapter.stream(input, controller.signal)) {
            await send({ protocol: 1, request_id: requestId, type: 'event', event: event satisfies WorkerEvent });
            if (event.payload.type === 'turn_finished') status = event.payload.status;
          }
          await send({ protocol: 1, request_id: requestId, type: 'reply', result: { status } });
        } catch { await send({ protocol: 1, request_id: requestId, type: 'error', code: 'provider_worker_failed' }); }
        finally { if (active?.requestId === requestId) active = undefined; }
      })();
      active = { requestId, controller, task };
    }
  } finally {
    active?.controller.abort();
    await active?.task;
    await writer;
  }
}
