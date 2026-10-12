// Typed transport for the durable Ironbark core. All wire shapes mirror
// crates/ironbark-core/src/contracts.rs (serde tag = "type", snake_case).
// The webview only ever sees typed values: the socket nonce, session handling
// and request correlation live in the native layer.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Webview-safe UUID source (browser WebCrypto; no Node in the webview). */
const uuid = (): string => crypto.randomUUID();

export interface Project {
  id: string;
  name: string;
  root: string;
  created_at: number;
}

/** Status {} reply value. */
export interface StatusReply {
  protocol: number;
  projects: number;
  last_seq: number;
  abandoned_requests: number;
}

/** Doctor {} reply value. */
export interface DoctorReply {
  protocol: number;
  sqlite_integrity: string;
  foreign_keys: boolean;
  schema_version: number;
  journal_mode: string;
}

export type CoreEventPayload =
  | { type: "state_changed"; entity_id: string; state: string }
  | { type: "artifact_added"; artifact_id: string; digest: string }
  | { type: "session_bound"; session: { kind: string; id: string } }
  | { type: string; [key: string]: unknown };

export interface EventEnvelope {
  protocol: number;
  seq: number;
  request_id?: string;
  project_id?: string;
  run_id?: string;
  timestamp: number;
  payload: CoreEventPayload;
}

export type CoreCommand =
  | { type: "status" }
  | { type: "events"; after: number }
  | { type: "project_list" }
  | { type: "project_create"; name: string; root: string }
  | { type: "doctor" };

export type ConnectionState = "connecting" | "live" | "error";

interface CoreReplyWire {
  type: "ok" | "error";
  value?: unknown;
  code?: string;
  message?: string;
}

/** The desktop surface of the core IPC protocol, wrapped over the three
 * Tauri commands. `status` uses the dedicated connect command; everything
 * else dispatches through `core_request` under a fresh request ID per call
 * (the core de-duplicates by request ID, so retries are safe). */
export class CoreClient {
  async status(): Promise<StatusReply> {
    const reply = await invoke<CoreReplyWire>("connect_core");
    return this.unwrap(reply) as StatusReply;
  }

  async doctor(): Promise<DoctorReply> {
    return this.call<DoctorReply>({ type: "doctor" });
  }

  async projectList(): Promise<Project[]> {
    return this.call<Project[]>({ type: "project_list" });
  }

  async events(after: number): Promise<EventEnvelope[]> {
    return this.call<EventEnvelope[]>({ type: "events", after });
  }

  /** Subscribe to durable events. Resolves with the unlisten; the native
   * layer reconnects from its high-water mark automatically. */
  async subscribe(after: number, onEvent: (e: EventEnvelope) => void, onState?: (state: ConnectionState) => void): Promise<UnlistenFn> {
    onState?.("connecting");
    const unlisten = await listen<EventEnvelope>("core-event", (ev) => onEvent(ev.payload));
    try {
      await invoke("subscribe_events", { afterSeq: after });
      onState?.("live");
    } catch (err) {
      onState?.("error");
      await unlisten();
      throw err;
    }
    return unlisten;
  }

  private async call<T>(command: CoreCommand): Promise<T> {
    const reply = await invoke<CoreReplyWire>("core_request", {
      requestId: uuid(),
      command,
    });
    return this.unwrap(reply) as T;
  }

  private unwrap(reply: CoreReplyWire): unknown {
    if (reply.type !== "ok") {
      throw new Error(`core ${reply.code}: ${reply.message}`);
    }
    return reply.value;
  }
}
