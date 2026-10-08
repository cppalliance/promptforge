// The agent contribution: the eager module registering the agent panel
// type, its New Agents Window row, and the chat pane's commands at module
// scope, before any service exists.
// The run body of New Agents Window opens a fresh agent panel keyed by a
// random instance id - each invocation is its own panel, socket, and modal
// server session in the right zone. main.ts imports zones.ts eagerly (it
// boots the dock through it), so the module is already loaded and the open
// call is direct; the agent chunk itself still loads lazily through the
// panel registry.
//
// The id is ours: Cursor ships a New Agents Window row but its command
// id is not public.
//
// The agent panel's type registers here too. Its instances key by the
// `instance` param; an open without one is the boot-time singleton. Its
// default title is "New Agent", and its tabs are renamable for the session.
//
// The chat pane's chords. Every one registers at the BuiltinExtension
// weight, so it beats the menu stubs that share a chord (the Go menu's Add
// Symbol rows, Toggle Developer Tools, Select All Occurrences), which
// register at the default WorkbenchContrib tier:
// - Ctrl+L and Ctrl+I, Open Chat: hide the pane when the chat has focus,
//   else reveal it and focus the composer.
// - Ctrl+Shift+L and Ctrl+Shift+I, New Chat: reuse an empty chat or open one.
// - Ctrl+T while a chat is active, and Ctrl+N while one is active and the
//   editor text is not focused: New Chat Tab, which reuses an empty chat too.
// - Ctrl+Shift+Backspace while a chat is active: composer.cancelComposerStep,
//   the stop behavior. Escape is never bound to stop.
// - Ctrl+Shift+Space while a chat is active: Voice Input, the mic's press.
// - Ctrl+W closes the chat tab; closing the last one hides the right zone.
// - Ctrl+[ and Ctrl+] cycle the chat tabs while a chat is active; with an
//   editor active they stay Outdent and Indent (the editor's own rules).
// - Ctrl+. opens the mode menu and Ctrl+/ the model menu (pressing the
//   mode chord again cycles).
// These chords are claimed everywhere - the dispatcher swallows a claimed
// chord even where its `when` fails - which is why the ones that other
// controls need (Shift+Tab) stay out of the registry.

import type { IDisposable } from "@workshop/platform/lifecycle";
import { registerAction, type ActionDescriptor } from "@workshop/platform/action-registry";
import type { ParseError } from "@workshop/platform/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { KeybindingsRegistry, KeybindingWeight } from "@workshop/platform/keybinding-registry";
import { MenuId, Menus } from "@workshop/platform/menu-registry";
import { registerPanelType } from "@workshop/platform/panel-registry";
import { onDidClosePanel } from "../layout/panel-close";
import { openInZone } from "../layout/zones";
import {
  AGENT_PANE_MENU,
  closeAllChatTabs,
  closeChatTab,
  closeOtherChatTabs,
  cycleChatTab,
  hidePaneWhenEmpty,
  newChat,
  openChat,
  openModeMenu,
  openModelMenu,
  renameChat,
  stopTurn,
  toggleVoiceInput,
} from "./agent-commands";

registerPanelType({
  type: "agent",
  title: "New Agent",
  defaultZone: "right",
  renamable: true,
  load: () => import("./index"),
});

/** The chat is the active panel. */
const AGENT_ACTIVE = "activeEditor == 'agent'";

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`agent action '${action.id}': ${result.error.message}`);
  }
}

/** Binds a further chord to an action that already owns its first. */
function addChord(id: string, keybinding: string, when?: string): void {
  KeybindingsRegistry.registerKeybindingRule({
    id,
    keybinding,
    ...(when === undefined ? {} : { when }),
    weight: KeybindingWeight.BuiltinExtension,
  });
}

addAction({
  id: "workbench.action.newAgentsWindow",
  title: "New Agents Window",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+alt+n" },
  menu: [{ id: MenuId.MenubarFileMenu, group: "1_new", order: 3 }],
  run: () => {
    openInZone("agent", { instance: window.crypto.randomUUID() });
  },
});

addAction({
  id: "workbench.action.chat.open",
  title: "Open Chat",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+l", weight: KeybindingWeight.BuiltinExtension },
  run: () => openChat(),
});
addChord("workbench.action.chat.open", "ctrlcmd+i");

addAction({
  id: "workbench.action.chat.new",
  title: "New Chat",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+shift+l", weight: KeybindingWeight.BuiltinExtension },
  run: () => newChat(),
});
addChord("workbench.action.chat.new", "ctrlcmd+shift+i");

addAction({
  id: "workbench.action.chat.newTab",
  title: "New Chat Tab",
  f1: true,
  keybinding: {
    keybinding: "ctrlcmd+t",
    when: AGENT_ACTIVE,
    weight: KeybindingWeight.BuiltinExtension,
  },
  run: () => newChat(),
});
// Ctrl+N is New Text File everywhere else, and stays that way while the
// editor's own text holds focus.
addChord("workbench.action.chat.newTab", "ctrlcmd+n", `${AGENT_ACTIVE} && !editorTextFocus`);

addAction({
  id: "composer.cancelComposerStep",
  title: "Stop",
  f1: true,
  keybinding: {
    keybinding: "ctrlcmd+shift+backspace",
    when: AGENT_ACTIVE,
    weight: KeybindingWeight.BuiltinExtension,
  },
  run: () => stopTurn(),
});

// The mic's tooltip advertises this chord: the round button is the mic only
// over an empty box, and a take over a draft starts from the keyboard.
addAction({
  id: "workbench.action.chat.toggleVoiceInput",
  title: "Voice Input",
  f1: true,
  keybinding: {
    keybinding: "ctrlcmd+shift+space",
    when: AGENT_ACTIVE,
    weight: KeybindingWeight.BuiltinExtension,
  },
  run: () => toggleVoiceInput(),
});

addAction({
  id: "composer.openModeMenu",
  title: "Switch Agent Mode",
  f1: true,
  precondition: AGENT_ACTIVE,
  keybinding: { keybinding: "ctrlcmd+.", weight: KeybindingWeight.BuiltinExtension },
  run: () => openModeMenu(),
});

addAction({
  id: "composer.openModelMenu",
  title: "Switch Model",
  f1: true,
  precondition: AGENT_ACTIVE,
  keybinding: { keybinding: "ctrlcmd+/", weight: KeybindingWeight.BuiltinExtension },
  run: () => openModelMenu(),
});

// Ctrl+] and Ctrl+[ are CodeMirror's indent chords too. The dispatcher
// swallows a claimed chord even where its `when` fails, so the editor owns
// them through its own rules (editor.contribution.ts: Indent Line and Outdent
// Line). The two sets never overlap: these need a chat as the active panel,
// those need an editor.
addAction({
  id: "workbench.action.chat.nextTab",
  title: "Next Chat Tab",
  f1: true,
  keybinding: {
    keybinding: "ctrlcmd+]",
    when: AGENT_ACTIVE,
    weight: KeybindingWeight.BuiltinExtension,
  },
  run: () => cycleChatTab(1),
});

addAction({
  id: "workbench.action.chat.previousTab",
  title: "Previous Chat Tab",
  f1: true,
  keybinding: {
    keybinding: "ctrlcmd+[",
    when: AGENT_ACTIVE,
    weight: KeybindingWeight.BuiltinExtension,
  },
  run: () => cycleChatTab(-1),
});

// The pane header's "..." menu: Toggle Chat Pane (the Secondary Side Bar's
// own command, relabeled), then the three close rows.
Menus.appendMenuItem(AGENT_PANE_MENU, {
  command: "workbench.action.toggleAuxiliaryBar",
  title: "Toggle Chat Pane",
  group: "1_pane",
  order: 1,
});

// The three close rows take the header's active chat as their `{ panelId }`
// argument (agent-pane-header.ts), so they work from the "..." menu whichever
// panel the dock has active.
addAction({
  id: "workbench.action.chat.closeTab",
  title: "Close Tab",
  keybinding: {
    keybinding: "ctrlcmd+w",
    when: AGENT_ACTIVE,
    weight: KeybindingWeight.BuiltinExtension,
  },
  menu: [{ id: AGENT_PANE_MENU, group: "2_close", order: 1 }],
  run: (arg?: unknown) => closeChatTab(arg),
});

addAction({
  id: "workbench.action.chat.closeOtherTabs",
  title: "Close Other Tabs",
  menu: [{ id: AGENT_PANE_MENU, group: "2_close", order: 2 }],
  run: (arg?: unknown) => closeOtherChatTabs(arg),
});

addAction({
  id: "workbench.action.chat.closeAllTabs",
  title: "Close All Tabs",
  menu: [{ id: AGENT_PANE_MENU, group: "2_close", order: 3 }],
  run: (arg?: unknown) => closeAllChatTabs(arg),
});

// The chat tab's own menu gains Rename Chat. The tab menu opens with the
// clicked tab's type as `activeEditor`, so the row shows for chat tabs only.
addAction({
  id: "workbench.action.chat.rename",
  title: "Rename Chat",
  menu: [
    { id: MenuId.EditorTitleContext, group: "2_chat", order: 1, when: AGENT_ACTIVE },
  ],
  run: (arg?: unknown) => renameChat(arg),
});

// Closing the last chat tab hides the pane.
onDidClosePanel(hidePaneWhenEmpty);
