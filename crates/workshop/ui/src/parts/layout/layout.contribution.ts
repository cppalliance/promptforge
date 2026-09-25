// The layout contribution: the eager module registering the sidebar and
// explorer catalog rows at module scope, before any service exists.
// Layout is a light feature - main.ts already pulls
// zones into the entry bundle - so the run bodies are direct calls.
//
// Explorer shows and focuses the Workshop tree. Primary Side Bar toggles
// the tree panel and mirrors the outcome into the sideBarVisible context
// key; Secondary Side Bar hides and shows the agent zone's dockview
// group through group.api.setVisible, so the agent panel's session
// socket survives - the panel is never removed - and mirrors the outcome
// into auxiliaryBarVisible. Both keys bind with a visible-by-default
// value on first toggle; layout's register() binds them at chunk load so
// the Appearance checkboxes read true from first paint.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import { CONTEXT_KEY_SERVICE } from "../../services/context-key-service";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { MenuId } from "../../services/menu-registry";
import { getService } from "../../services/service-registry";
import { focusWorkshopTree, toggleWorkshopPanel } from "./workshop-panel";
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

/** Mirrors a visibility outcome into its context key (both default visible). */
function setVisibilityKey(name: string, visible: boolean): void {
  getService(CONTEXT_KEY_SERVICE).createKey(name, true).set(visible);
}

addAction({
  id: "workbench.view.explorer",
  title: "Explorer",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+shift+e" },
  menu: [{ id: MenuId.MenubarViewMenu, group: "3_views", order: 1 }],
  run: focusWorkshopTree,
});

addAction({
  id: "workbench.action.toggleSidebarVisibility",
  title: "Primary Side Bar",
  f1: true,
  toggled: "sideBarVisible",
  keybinding: { keybinding: "ctrlcmd+b" },
  menu: [{ id: APPEARANCE_MENU, group: "2_workbench_layout", order: 2 }],
  run: () => {
    setVisibilityKey("sideBarVisible", toggleWorkshopPanel());
  },
});

addAction({
  id: "workbench.action.toggleAuxiliaryBar",
  title: "Secondary Side Bar",
  f1: true,
  toggled: "auxiliaryBarVisible",
  keybinding: { keybinding: "ctrlcmd+alt+b" },
  menu: [{ id: APPEARANCE_MENU, group: "2_workbench_layout", order: 3 }],
  run: () => {
    // A hidden group stays live, so the toggle always finds it; a zone
    // whose group was never built opens the agent panel instead.
    const visible = toggleZoneVisibility("right");
    if (visible === undefined) {
      openInZone("agent", {});
      setVisibilityKey("auxiliaryBarVisible", true);
      return;
    }
    setVisibilityKey("auxiliaryBarVisible", visible);
  },
});
