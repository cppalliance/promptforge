// The seam between the agent contribution's commands and the agent panel.
// The contribution is eager and the panel is a lazy chunk, so the
// contribution cannot import the panel class (or test instanceof against
// it); it reaches a loaded panel through this structural handle instead,
// found by duck-typing the dock's resolved panel content. A panel whose
// chunk has not loaded yet is simply not a handle: the commands then have
// nothing to act on, which is the right answer for a chat that does not
// exist yet.

import type { IContentRenderer } from "dockview";

import { resolvePanelContent } from "@workshop/platform/panel-registry";

/** What a command can ask of one agent panel. */
export interface AgentPanelHandle {
  /** Focuses the composer's editor. */
  focusInput(): void;
  /** Whether focus sits inside the panel. */
  hasFocus(): boolean;
  /** Whether the chat has no turn yet and no draft: one to reuse instead of opening another. */
  isEmpty(): boolean;
  /** Cancels the running turn; false when none runs or the socket is down. */
  cancelTurn(): boolean;
  /** Starts or ends a dictation take, as the mic button does over an empty box. */
  toggleVoiceInput(): void;
  /** Opens the mode menu, or cycles to the next mode while it is open. */
  openModeMenu(): void;
  /** Opens the model menu. */
  openModelMenu(): void;
}

const HANDLE_METHODS = [
  "focusInput",
  "hasFocus",
  "isEmpty",
  "cancelTurn",
  "toggleVoiceInput",
  "openModeMenu",
  "openModelMenu",
] as const;

/** The agent panel handle behind a dock panel's content renderer, or null while its chunk is loading. */
export function agentHandleOf(content: IContentRenderer): AgentPanelHandle | null {
  const resolved: unknown = resolvePanelContent(content);
  if (typeof resolved !== "object" || resolved === null) {
    return null;
  }
  for (const method of HANDLE_METHODS) {
    if (typeof (resolved as Record<string, unknown>)[method] !== "function") {
      return null;
    }
  }
  return resolved as AgentPanelHandle;
}
