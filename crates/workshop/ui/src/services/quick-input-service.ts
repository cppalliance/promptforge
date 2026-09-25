// The quick input service contract and its provider vocabulary. The
// implementation (parts/quickinput/quick-input.ts) is a DOM widget - the
// floating panel under the title bar - so it stays in parts; the row,
// provider, and show-options shapes, the service interface, and the token
// live here, in the DOM-free services layer, so menu and quick-access
// contributions can name the contract without pulling the widget chunk.

import { createServiceToken, type ServiceToken } from "./service-registry";

/** One row in the quick input list. */
export interface QuickInputItem {
  /** The row's primary text. */
  readonly label: string;
  /** Secondary muted text, e.g. a path or a category. */
  readonly description?: string;
  /** The keybinding hint shown at the row's right edge. */
  readonly keybinding?: string;
  /** Runs the row's action. The panel has already closed. */
  accept(): void;
}

/**
 * The provider shape the widget expects a descriptor's factory to
 * produce. The registry never calls the factory; this interface is the
 * widget's side of the contract.
 */
export interface QuickAccessProvider {
  /** The rows for `filter` (the input value minus the prefix). */
  getItems(filter: string): readonly QuickInputItem[];
}

/** Options for one quick input showing. */
export interface QuickInputShowOptions {
  /**
   * Render every provider's help entries above the active provider's
   * rows while the input is empty - the modes list.
   */
  readonly includeHelp?: boolean;
}

/** The quick input service consumers resolve from the registry. */
export interface QuickInputService {
  /** The quick-access surface the menu actions and command center call. */
  readonly quickAccess: {
    show(value: string, options?: QuickInputShowOptions): void;
  };
}

/** The service token the composition root registers the widget under. */
export const QUICK_INPUT_SERVICE: ServiceToken<QuickInputService> =
  createServiceToken<QuickInputService>("workshop.quickInput");
