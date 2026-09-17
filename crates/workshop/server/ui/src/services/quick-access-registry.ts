// The quick-access registry: the module-level store of quick-access
// provider descriptors (the shape of VS Code's
// IQuickAccessProviderDescriptor) that feature contribution files write
// at module scope and the quick input widget reads to route its input.
// Each descriptor names the prefix that claims the input ("" is the
// default file provider, ">" the command palette), the placeholder the
// input shows while the provider is active, the help entries the ?
// mode lists, and a factory that builds the provider lazily.
//
// Routing is longest-prefix: the registered prefix that is a prefix of
// the input value and longer than every other match wins, so "debug "
// beats "debug" beats "" regardless of registration order. Registration
// upserts by prefix and returns a disposable that removes only its own
// registration, so disposing a replaced provider keeps its replacement.
//
// The registry never calls the factory; the quick input widget owns the
// provider shape and narrows what the factory returns.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { toDisposable, type IDisposable } from "../base/lifecycle";

/** One row in the ? help list. */
export interface QuickAccessHelpEntry {
  /** What the mode does, e.g. "Show and Run Commands". */
  readonly description: string;
  /** The prefix that enters the mode. */
  readonly prefix: string;
}

/** A quick-access provider registration, keyed by its prefix. */
export interface QuickAccessProviderDescriptor {
  /** The input prefix that routes to this provider; "" is the default. */
  readonly prefix: string;
  /** The placeholder text the input shows while this provider is active. */
  readonly placeholder: string;
  /** The rows this provider contributes to the ? help list. */
  readonly helpEntries: readonly QuickAccessHelpEntry[];
  /**
   * Builds the provider lazily. The registry stores the factory and
   * never calls it; the quick input widget owns the provider shape.
   */
  readonly factory: () => unknown;
}

/** The provider store the quick input widget reads. */
export interface QuickAccessRegistry {
  /**
   * Registers one provider. A second registration with the same prefix
   * replaces the first. The returned disposable removes only this
   * registration.
   */
  registerQuickAccessProvider(descriptor: QuickAccessProviderDescriptor): IDisposable;
  /**
   * The descriptor whose prefix is the longest prefix of `value`, or
   * undefined when no prefix matches (possible only without a ""
   * provider, since "" is a prefix of every value).
   */
  getQuickAccessProvider(value: string): QuickAccessProviderDescriptor | undefined;
  /** Every registered provider, feeding the ? help list. */
  getQuickAccessProviders(): readonly QuickAccessProviderDescriptor[];
}

/** Builds an empty registry. */
export function createQuickAccessRegistry(): QuickAccessRegistry {
  const providers: QuickAccessProviderDescriptor[] = [];

  function registerQuickAccessProvider(descriptor: QuickAccessProviderDescriptor): IDisposable {
    const existing = providers.findIndex((provider) => provider.prefix === descriptor.prefix);
    if (existing !== -1) {
      providers.splice(existing, 1);
    }
    providers.push(descriptor);
    return toDisposable(() => {
      const index = providers.indexOf(descriptor);
      if (index !== -1) {
        providers.splice(index, 1);
      }
    });
  }

  return {
    registerQuickAccessProvider,
    getQuickAccessProvider(value: string): QuickAccessProviderDescriptor | undefined {
      let best: QuickAccessProviderDescriptor | undefined;
      for (const provider of providers) {
        if (value.startsWith(provider.prefix) && (best === undefined || provider.prefix.length > best.prefix.length)) {
          best = provider;
        }
      }
      return best;
    },
    getQuickAccessProviders(): readonly QuickAccessProviderDescriptor[] {
      return providers.slice();
    },
  };
}

/** The shared registry the running app's contributions populate. */
export const QuickAccessRegistry: QuickAccessRegistry = createQuickAccessRegistry();
