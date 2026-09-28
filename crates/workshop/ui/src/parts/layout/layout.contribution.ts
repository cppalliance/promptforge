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

import type { IDisposable } from "@workshop/platform/lifecycle";
import { registerAction, type ActionDescriptor } from "@workshop/platform/action-registry";
import { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
import type { ParseError } from "@workshop/platform/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { LAYOUT_POLICY } from "../../services/layout-policy";
import { MenuId } from "@workshop/platform/menu-registry";
import { panelTypeEntry } from "@workshop/platform/panel-registry";
import { getService } from "@workshop/platform/service-registry";
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
    // A hidden group stays live, so the toggle always finds it; a zone
    // whose group was never built opens the layout policy's right-zone
    // anchors instead.
    const visible = toggleZoneVisibility("right");
    if (visible === undefined) {
      for (const anchor of getService(LAYOUT_POLICY).anchors) {
        if (panelTypeEntry(anchor)?.defaultZone === "right") {
          openInZone(anchor, {});
        }
      }
      setVisibilityKey("auxiliaryBarVisible", true);
      return;
    }
    setVisibilityKey("auxiliaryBarVisible", visible);
  },
});
