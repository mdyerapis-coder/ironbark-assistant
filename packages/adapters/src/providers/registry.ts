import type { ConnectionConfig, ConnectionDescriptor, WorkerProviderTurnInput } from '../../../contracts/src/generated.ts';
import { validateEndpoint } from './local.ts';

export const SDK_APIS = {
  openai_completions: 'openai-completions', openai_responses: 'openai-responses',
  anthropic: 'anthropic-messages', gemini: 'google-generative-ai',
  ollama: 'ollama-chat', llama_cpp: 'openai-completions',
} as const;
export class ProviderRegistry {
  private readonly connections = new Map<string, ConnectionConfig>();
  constructor(connections: ConnectionConfig[]) {
    for (const connection of connections) {
      if (this.connections.has(connection.id)) throw new Error('duplicate_connection');
      validateEndpoint(connection.endpoint, false);
      this.connections.set(connection.id, structuredClone(connection));
    }
  }
  describe(connection: ConnectionConfig): ConnectionDescriptor {
    validateEndpoint(connection.endpoint, false);
    return { id: connection.id, protocol: connection.protocol, endpoint: connection.endpoint,
      models: structuredClone(connection.models), capabilities: [], availability: 'unqualified', auth_available: false };
  }
  select(input: WorkerProviderTurnInput, customVersion?: string) {
    const connection = this.connections.get(input.route.connection_id);
    if (!connection || input.connection.protocol !== connection.protocol || input.route.harness || !input.policy.allowed_connection_ids.includes(connection.id)
      || !connection.allowed_projects.includes(input.policy.project_id)) throw new Error('route_denied');
    validateEndpoint(connection.endpoint, input.policy.local_only);
    if (connection.protocol === 'custom' && !customVersion) throw new Error('custom_activation_required');
    if (connection.protocol !== 'custom' && !Object.hasOwn(SDK_APIS, connection.protocol)) throw new Error('unsupported_protocol');
    const api = connection.protocol === 'custom' ? undefined : SDK_APIS[connection.protocol as keyof typeof SDK_APIS];
    const model = connection.models.find(candidate => candidate.id === input.route.model_id);
    if (!model || !Number.isSafeInteger(model.context_limit) || model.context_limit <= 0
      || !Number.isSafeInteger(model.output_limit) || model.output_limit <= 0) throw new Error('unknown_model');
    const required = new Set<string>(['text', ...input.required_capabilities]);
    if (input.tool_schemas.length || input.messages.some(message => message.role === 'tool' || message.tool_calls?.length)) required.add('tools');
    for (const message of input.messages) for (const part of message.content) {
      if (part.type === 'image') required.add('vision');
      else if (part.type !== 'text') throw new Error('unsupported_content');
    }
    for (const capability of required) {
      if (!['text', 'tools', 'vision', 'reasoning'].includes(capability)) throw new Error('unsupported_capability');
      if (!model.capabilities.some(observation => observation.name === capability && observation.state === 'qualified'
        && observation.evidence_digest && observation.implementation_version === (customVersion ?? '18.8.9'))) throw new Error('unqualified_capability');
    }
    if (input.max_output_tokens !== undefined && (!Number.isSafeInteger(input.max_output_tokens)
      || input.max_output_tokens <= 0 || input.max_output_tokens > model.output_limit)) throw new Error('output_limit_denied');
    return { connection, model, api };
  }
}
