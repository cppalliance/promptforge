// The quick input service contract and its provider vocabulary. The
// implementation (parts/quickinput/quick-input.ts) is a DOM widget - the
// floating panel under the title bar - so it stays in parts; the row,
// provider, and show-options shapes, the service interface, the token, and
// the substring highlighter the providers share live here, in the DOM-free
// services layer, so menu and quick-access contributions can name the
// contract without pulling the widget chunk.

import { createServiceToken, type ServiceToken } from "@workshop/platform/service-registry";

/** A span of a row's label drawn as a filter match: `[start, end)` character offsets. */
export interface LabelHighlight {
  readonly start: number;
  readonly end: number;
}

/**
 * The highlight a case-insensitive substring filter leaves on `text`: its
 * first occurrence. Answers undefined for an empty needle or no match, so a
 * row carries a highlight only when there is something to draw.
 */
export function substringHighlights(text: string, needle: string): readonly LabelHighlight[] | undefined {
  if (needle === "") {
    return undefined;
  }
  const lowered = text.toLowerCase();
  // A lowercase form of another length would shift every offset after it.
  if (lowered.length !== text.length) {
    return undefined;
  }
  const start = lowered.indexOf(needle.toLowerCase());
  return start === -1 ? undefined : [{ start, end: start + needle.length }];
}

/** One row in the quick input list. */
export interface QuickInputItem {
  /** The row's primary text. */
  readonly label: string;
  /** Spans of the label drawn as filter matches. */
  readonly labelHighlights?: readonly LabelHighlight[];
  /** Secondary muted text, e.g. a path or a category. */
  readonly description?: string;
  /** The keybinding hint shown at the row's right edge, e.g. "Ctrl+Shift+P". */
  readonly keybinding?: string;
  /** A group label drawn above this row, e.g. "recently used". */
  readonly separator?: string;
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
  /** What the list says when getItems answers no rows; without it the list stays blank. */
  readonly noResultsMessage?: string;
  /**
   * Sends the input to another mode: answers the value to put in the input
   * in place of the current one, or undefined to stay. The help provider
   * uses it so typing a mode's prefix after ? enters that mode.
   */
  redirect?(filter: string): string | undefined;
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
