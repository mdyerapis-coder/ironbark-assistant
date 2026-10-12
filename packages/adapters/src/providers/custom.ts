import type { ProviderAdapter } from '../../../contracts/src/generated.ts';

// Extensions are trusted code and can only be installed by the native launcher.
// A JSON connection or endpoint never imports executable code.
export interface ActivatedCustomAdapter { adapter: ProviderAdapter; implementationVersion: string }
export class CustomAdapters {
  private readonly adapters = new Map<string, ActivatedCustomAdapter>();
  activate(connectionId: string, adapter: ProviderAdapter, implementationVersion: string, approved: boolean) {
    if (!approved || !/^[a-z0-9][a-z0-9.-]{0,63}$/.test(connectionId) || !implementationVersion || this.adapters.has(connectionId)) {
      throw new Error('custom_activation_denied');
    }
    this.adapters.set(connectionId, { adapter, implementationVersion });
  }
  get(connectionId: string) { return this.adapters.get(connectionId); }
}
