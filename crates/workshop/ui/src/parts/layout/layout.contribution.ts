// The layout contribution: the module loaded eagerly to register the
// Secondary Side Bar catalog row at module scope, before any service
// exists. Layout is a light feature - main.ts already loads zones
// eagerly - so the run body is a direct call.
//
// Secondary Side Bar hides and shows the right zone's dockview group
// through group.api.setVisible, so its panels and their live connections
// survive - nothing is removed - and mirrors the outcome into
// auxiliaryBarVisible. The key binds with a visible-by-default value on
// first toggle; the workspace directory's register() binds it at chunk
// load so the Appearance checkbox reads true from first paint.
//
// Close and Close Others run the layout core's close path in
// panel-close.ts over every panel type, so Close keeps the bare
// activeEditor precondition. Both take an optional { panelId } argument
// naming the panel to act on, else the active one; the generic tab menu
// passes the clicked tab's. Its Close row is appended directly so it
// reads Close, not the command's Close Editor.

import type { IDisposable } from "@workshop/platform/lifecycle";
import { registerAction, type ActionDescriptor } from "@workshop/platform/action-registry";
import { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
import type { ParseError } from "@workshop/platform/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { LAYOUT_POLICY } from "../../services/layout-policy";
import { KeybindingWeight } from "@workshop/platform/keybinding-registry";
import { MenuId, Menus } from "@workshop/platform/menu-registry";
import { panelTypeEntry } from "@workshop/platform/panel-registry";
import { getService } from "@workshop/platform/service-registry";
import { closeActiveEditor, closeOtherEditors } from "./panel-close";
import { openInZone, toggleZoneVisibility } from "./zones";

/** The Appearance flyout's id; the menubar contribution declares the submenu. */
const APPEARANCE_MENU: MenuId = "menubar/view/appearance";

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`layout action '${action.id}': ${result.error.message}`);
  }
}

/** Mirrors a visibility outcome into its context key (default visible). */
function setVisibilityKey(name: string, visible: boolean): void {
  getService(CONTEXT_KEY_SERVICE).createKey(name, true).set(visible);
}

addAction({
  id: "workbench.action.toggleAuxiliaryBar",
  title: "Secondary Side Bar",
  f1: true,
  toggled: "auxiliaryBarVisible",
  keybinding: { keybinding: "ctrlcmd+alt+b" },
  menu: [{ id: APPEARANCE_MENU, group: "2_workbench_layout", order: 3 }],
  run: () => {
    // A hidden group stays live, so the toggle always finds it, and it
    // writes the auxiliaryBarVisible key itself; a zone whose group was
    // never built opens the layout policy's right-zone anchors instead.
    if (toggleZoneVisibility("right") !== undefined) {
      return;
    }
    for (const anchor of getService(LAYOUT_POLICY).anchors) {
      if (panelTypeEntry(anchor)?.defaultZone === "right") {
        openInZone(anchor, {});
      }
    }
    setVisibilityKey("auxiliaryBarVisible", true);
  },
});

addAction({
  id: "workbench.action.closeActiveEditor",
  title: "Close Editor",
  f1: true,
  precondition: "activeEditor",
  keybinding: { keybinding: "ctrlcmd+f4", when: "editorTextFocus", weight: KeybindingWeight.WorkbenchContrib },
  menu: [{ id: MenuId.MenubarFileMenu, group: "6_close", order: 2 }],
  run: async (arg?: unknown) => {
    await closeActiveEditor(arg);
  },
});
Menus.appendMenuItem(MenuId.EditorTitleContext, {
  command: "workbench.action.closeActiveEditor",
  title: "Close",
  group: "1_close",
  order: 1,
});

addAction({
  id: "workbench.action.closeOtherEditors",
  title: "Close Others",
  menu: [{ id: MenuId.EditorTitleContext, group: "1_close", order: 2 }],
  run: (arg?: unknown) => closeOtherEditors(arg),
});
