// The quickinput contribution: the eager module that registers the
// quick-access providers (the ">" palette, the "?" help list, and the
// "@", "%", "debug ", "task " placeholders, in modes-list order) and
// the four quick-access actions at module scope, before any service
// exists. The run bodies resolve the quick input service at call time,
// so the actions work regardless of when the composition root builds
// the widget.
//
// Menu placements follow the catalog: showCommands sits in View (as
// "Command Palette...") and Help (as "Show All Commands", a second
// placement with its own title), quickOpen in Go; quickOpenWithModes is
// the command-center pill's row and quickOpenHelp its chevron, so
// neither is f1. The ctrl-based chords bind ctrlcmd so macOS gets
// Cmd+Shift+P and Cmd+P; showCommands' second chord (F1) is a separate
// rule, and the first registered rule owns the palette's keybinding
// label.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { KeybindingsRegistry } from "../../services/keybinding-registry";
import { appendMenuItem, MenuId } from "../../services/menu-registry";
import { QuickAccessRegistry } from "../../services/quick-access-registry";
import { getService } from "../../services/service-registry";
import { createQuickAccessProviderDescriptors } from "./quick-access-providers";
import { QUICK_INPUT_SERVICE, type QuickInputShowOptions } from "./quick-input";

/** Opens quick input at `value`; resolves the widget at call time. */
function showQuickInput(value: string, options?: QuickInputShowOptions): void {
  getService(QUICK_INPUT_SERVICE).quickAccess.show(value, options);
}

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`quickinput action '${action.id}': ${result.error.message}`);
  }
}

for (const descriptor of createQuickAccessProviderDescriptors()) {
  QuickAccessRegistry.registerQuickAccessProvider(descriptor);
}

addAction({
  id: "workbench.action.showCommands",
  title: "Command Palette...",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+shift+p" },
  menu: [{ id: MenuId.MenubarViewMenu, group: "1_open", order: 1 }],
  run: () => showQuickInput(">"),
});
KeybindingsRegistry.registerKeybindingRule({ id: "workbench.action.showCommands", keybinding: "f1" });
appendMenuItem(MenuId.MenubarHelpMenu, {
  command: "workbench.action.showCommands",
  title: "Show All Commands",
  group: "1_welcome",
  order: 1,
});

addAction({
  id: "workbench.action.quickOpen",
  title: "Go to File...",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+p" },
  menu: [{ id: MenuId.MenubarGoMenu, group: "3_global_nav", order: 1 }],
  run: () => showQuickInput(""),
});

addAction({
  id: "workbench.action.quickOpenWithModes",
  title: "Quick Open With Modes",
  menu: [{ id: MenuId.CommandCenter }],
  run: () => showQuickInput("", { includeHelp: true }),
});

addAction({
  id: "workbench.action.quickOpenHelp",
  title: "Quick Open Help",
  run: () => showQuickInput("?"),
});
