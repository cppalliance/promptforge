// The agent pane's commands, as plain functions over the dock: open or hide
// the chat, start a chat (reusing an empty one), stop the running turn,
// close and cycle the chat tabs, rename one, and open the composer's menus.
// agent.contribution.ts registers them as actions with their keybindings;
// the pane header's buttons and its "..." menu run the same actions.
//
// This module is eager and the panel is a lazy chunk, so every command
// reaches a chat through the dock and its structural handle
// (agent-handle.ts), never the panel class. A chat whose chunk has not
// loaded yet has no handle, and the commands that need one wait for the
// chunk (loadPanelType) before they act.

import type { IDockviewPanel } from "dockview";

import { DOCK, loadPanelType } from "@workshop/platform/panel-registry";
import { getService } from "@workshop/platform/service-registry";
import { closeActiveEditor, closeOtherEditors } from "../layout/panel-close";
import { setTabName, tabNameOf } from "../layout/tab-names";
import { groupOfZone, openInZone, setZoneVisibility, zoneOfPanel } from "../layout/zones";
import { showPanelDialog } from "../shared/panel-dialog";
import { agentHandleOf, type AgentPanelHandle } from "./agent-handle";

/** The panel type the chat tabs are. */
const AGENT_TYPE = "agent";

/** The pane header's "..." menu. */
export const AGENT_PANE_MENU = "workbench/agentPane/more";

/** Every chat tab in the dock, in dock order. */
export function agentPanels(): IDockviewPanel[] {
  return getService(DOCK).panels.filter((panel) => panel.api.component === AGENT_TYPE);
}

/** The loaded handle behind a chat tab, or null while its chunk is loading. */
function handleOf(panel: IDockviewPanel): AgentPanelHandle | null {
  return agentHandleOf(panel.view.content);
}

/** The active panel when it is a chat tab. */
function activeAgentPanel(): IDockviewPanel | undefined {
  const active = getService(DOCK).activePanel;
  return active !== undefined && active.api.component === AGENT_TYPE ? active : undefined;
}

/** The chat tab a `{ panelId }` argument names, else the active one. */
function targetPanel(arg: unknown): IDockviewPanel | undefined {
  if (typeof arg === "object" && arg !== null && "panelId" in arg && typeof arg.panelId === "string") {
    const named = getService(DOCK).getPanel(arg.panelId);
    return named !== undefined && named.api.component === AGENT_TYPE ? named : undefined;
  }
  return activeAgentPanel();
}

/**
 * Focuses a chat's composer, waiting for the chunk when the chat is still
 * loading. A failed chunk load is dropped on purpose: the chat's own
 * placeholder shares this load (loadPanelType memoizes the in-flight promise)
 * and already shows the error in the pane (LazyPanel in panel-types.ts), and
 * with no panel there is nothing to focus. A throw from `focusInput` is not a
 * load failure and is left to surface.
 */
function focusChat(panel: IDockviewPanel): void {
  void loadPanelType(AGENT_TYPE).then(
    () => {
      handleOf(panel)?.focusInput();
    },
    () => undefined,
  );
}

/** Shows the right zone when the chat pane was hidden. */
function revealChatPane(): void {
  const group = groupOfZone("right");
  if (group !== undefined && !group.api.isVisible) {
    setZoneVisibility("right", true);
  }
}

/** Hides the chat pane (the right zone's group stays live, with its tabs). */
export function hideChatPane(): void {
  setZoneVisibility("right", false);
}

/** A fresh chat tab in the right zone. */
function openNewChat(): IDockviewPanel {
  return openInZone(AGENT_TYPE, { instance: window.crypto.randomUUID() });
}

/**
 * Open Chat (Ctrl+L, Ctrl+I): with focus in a chat, hides the pane;
 * otherwise reveals it, makes a chat active (opening one when none
 * exists), and focuses its composer.
 */
export function openChat(): void {
  const chats = agentPanels();
  if (chats.some((panel) => handleOf(panel)?.hasFocus() === true)) {
    hideChatPane();
    return;
  }
  const group = groupOfZone("right");
  const active = group?.activePanel;
  const target =
    active !== undefined && active.api.component === AGENT_TYPE
      ? active
      : chats.find((panel) => zoneOfPanel(panel) === "right");
  if (target === undefined) {
    focusChat(openNewChat());
    return;
  }
  revealChatPane();
  target.api.setActive();
  focusChat(target);
}

/**
 * New Chat and New Chat Tab (Ctrl+Shift+L, Ctrl+Shift+I, Ctrl+T, Ctrl+N,
 * and the header's +): reuses the active chat when it is empty, else the
 * first empty one, else opens a new tab. The pane is revealed and the
 * composer focused either way.
 */
export function newChat(): void {
  const chats = agentPanels();
  const active = activeAgentPanel();
  const empties = chats.filter((panel) => handleOf(panel)?.isEmpty() === true);
  const reused = active !== undefined && empties.includes(active) ? active : empties[0];
  if (reused === undefined) {
    focusChat(openNewChat());
    return;
  }
  revealChatPane();
  reused.api.setActive();
  focusChat(reused);
}

/** Stop (Ctrl+Shift+Backspace): cancels the active chat's running turn. */
export function stopTurn(): void {
  const active = activeAgentPanel();
  if (active !== undefined) {
    handleOf(active)?.cancelTurn();
  }
}

/** Voice Input (Ctrl+Shift+Space): starts or ends a dictation take in the active chat. */
export function toggleVoiceInput(): void {
  const active = activeAgentPanel();
  if (active !== undefined) {
    handleOf(active)?.toggleVoiceInput();
  }
}

/** Ctrl+. and Ctrl+/ : the active chat's mode and model menus. */
export function openModeMenu(): void {
  const active = activeAgentPanel();
  if (active !== undefined) {
    handleOf(active)?.openModeMenu();
  }
}

export function openModelMenu(): void {
  const active = activeAgentPanel();
  if (active !== undefined) {
    handleOf(active)?.openModelMenu();
  }
}

/** Ctrl+[ and Ctrl+]: the next or previous chat tab in the active chat's group, wrapping. */
export function cycleChatTab(step: 1 | -1): void {
  const active = activeAgentPanel();
  if (active === undefined) {
    return;
  }
  const chats = active.group.panels.filter((panel) => panel.api.component === AGENT_TYPE);
  const index = chats.indexOf(active);
  const next = chats[(index + step + chats.length) % chats.length];
  if (next !== undefined && next !== active) {
    next.api.setActive();
    focusChat(next);
  }
}

/** Close Tab (Ctrl+W): closes the named chat tab, else the active one. */
export async function closeChatTab(arg?: unknown): Promise<void> {
  const target = targetPanel(arg);
  if (target !== undefined) {
    await closeActiveEditor({ panelId: target.id });
  }
}

/** Close Other Tabs: closes the group's other tabs around the named chat, else the active one. */
export async function closeOtherChatTabs(arg?: unknown): Promise<void> {
  const target = targetPanel(arg);
  if (target !== undefined) {
    await closeOtherEditors({ panelId: target.id });
  }
}

/** Close All Tabs: closes every chat tab in the active chat's group, one at a time. */
export async function closeAllChatTabs(arg?: unknown): Promise<void> {
  const target = targetPanel(arg);
  if (target === undefined) {
    return;
  }
  const chats = target.group.panels.filter((panel) => panel.api.component === AGENT_TYPE);
  for (const panel of chats) {
    if (!(await closeActiveEditor({ panelId: panel.id }))) {
      return;
    }
  }
}

/**
 * Closing the last chat tab hides the right zone. The close path announces
 * each panel it closes; when a chat was the last thing in the zone, the
 * rebuilt empty group is hidden, not left as a blank strip.
 */
export function hidePaneWhenEmpty(closed: IDockviewPanel): void {
  if (closed.api.component !== AGENT_TYPE) {
    return;
  }
  const group = groupOfZone("right");
  if (group !== undefined && group.panels.length === 0 && group.api.isVisible) {
    hideChatPane();
  }
}

/**
 * Rename Chat (the tab menu): asks for a name in the form modal, prefilled
 * with the current one. The name lasts for the session only.
 */
export function renameChat(arg?: unknown): void {
  const target = targetPanel(arg);
  if (target === undefined) {
    return;
  }
  const panelId = target.id;
  showPanelDialog({
    container: document.body,
    classPrefix: "ws-chat-rename",
    titleId: "ws-chat-rename-title",
    title: "Rename Chat",
    message: "Enter new chat name",
    skin: "form",
    field: {
      id: "ws-chat-rename-name",
      label: "Chat name",
      placeholder: "Chat name",
      value: tabNameOf(panelId) ?? target.api.title ?? "",
    },
    buttons: [
      { label: "Cancel", run: () => undefined },
      {
        label: "Rename",
        primary: true,
        requiresValue: true,
        run: (name) => setTabName(panelId, name),
      },
    ],
  });
}
