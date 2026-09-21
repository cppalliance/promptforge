// The chrome contribution: the eager module registering the window-level
// catalog rows (plan step 19) at module scope, before any service exists.
// Chrome is a light eager feature - main.ts already pulls window-chrome
// and zoom into the entry bundle - so the run bodies are direct calls.
//
// Placements follow the catalog: the zoom triple sits in Appearance's
// 5_zoom group with ctrlcmd chords (Cmd on macOS), Full Screen and Close
// Window are desktop-only (precondition !isWeb), and About lands at the
// Help menu's z_about. Reset Zoom's second chord (ctrlcmd+0) and the two
// per-OS overrides (Full Screen's ctrl+meta+f on macOS, Close Window's
// meta+shift+w) are separate registerKeybindingRule calls: the action
// descriptor's keybinding omits the mac field, so those rules register
// directly with the precondition ANDed in by hand. Zoom In also binds
// ctrlcmd+shift+= - the shifted plus is the same physical key, and both
// are the conventional zoom-in chord. The first rule registered for a
// command becomes its menu label, so Reset Zoom shows Ctrl+NumPad0.

import type { IDisposable } from "../../base/lifecycle";
import { invoke } from "@tauri-apps/api/core";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { KeybindingsRegistry } from "../../services/keybinding-registry";
import { MenuId } from "../../services/menu-registry";
import { showAboutDialog } from "./about-dialog";
import { closeWindow, toggleFullScreen } from "./window-chrome";
import { resetZoom, zoomIn, zoomOut } from "./zoom";

/** The Appearance flyout's id; the menubar contribution declares the submenu. */
const APPEARANCE_MENU: MenuId = "menubar/view/appearance";

/** Registers one action, reporting a malformed descriptor instead of throwing. */
function addAction(action: ActionDescriptor): void {
  const result: Result<IDisposable, ParseError> = registerAction(action);
  if (!result.ok) {
    console.error(`chrome action '${action.id}': ${result.error.message}`);
  }
}

addAction({
  id: "workbench.action.toggleFullScreen",
  title: "Full Screen",
  f1: true,
  precondition: "!isWeb",
  toggled: "isFullscreen",
  menu: [{ id: APPEARANCE_MENU, group: "1_toggle_view", order: 1 }],
  run: toggleFullScreen,
});
KeybindingsRegistry.registerKeybindingRule({
  id: "workbench.action.toggleFullScreen",
  keybinding: "f11",
  mac: "ctrl+meta+f",
  when: "!isWeb",
});

addAction({
  id: "workbench.action.closeWindow",
  title: "Close Window",
  f1: true,
  precondition: "!isWeb",
  menu: [{ id: MenuId.MenubarFileMenu, group: "6_close", order: 4 }],
  run: closeWindow,
});
KeybindingsRegistry.registerKeybindingRule({
  id: "workbench.action.closeWindow",
  keybinding: "alt+f4",
  mac: "meta+shift+w",
  when: "!isWeb",
});

addAction({
  id: "workbench.action.zoomIn",
  title: "Zoom In",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+=" },
  menu: [{ id: APPEARANCE_MENU, group: "5_zoom", order: 1 }],
  run: zoomIn,
});
KeybindingsRegistry.registerKeybindingRule({
  id: "workbench.action.zoomIn",
  keybinding: "ctrlcmd+shift+=",
});

addAction({
  id: "workbench.action.zoomOut",
  title: "Zoom Out",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+-" },
  menu: [{ id: APPEARANCE_MENU, group: "5_zoom", order: 2 }],
  run: zoomOut,
});

addAction({
  id: "workbench.action.zoomReset",
  title: "Reset Zoom",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+numpad0" },
  menu: [{ id: APPEARANCE_MENU, group: "5_zoom", order: 3 }],
  run: resetZoom,
});
KeybindingsRegistry.registerKeybindingRule({
  id: "workbench.action.zoomReset",
  keybinding: "ctrlcmd+0",
});

addAction({
  id: "workbench.action.showAboutDialog",
  title: "About",
  f1: true,
  menu: [{ id: MenuId.MenubarHelpMenu, group: "z_about" }],
  run: () => {
    showAboutDialog();
  },
});

// File > Exit (plan step 20's menu assembly; the catalog's chrome row).
// The run body invokes the shell's quit command - the same
// gateway-shutdown-then-exit path the native menu's quit item runs -
// which the shell step lands in promptforge/crates/workshop; until then
// an activation rejects and surfaces on the status bar, never
// plugin-process exit(0), which would strand the sidecar gateway.
// Desktop-only: the !isWeb precondition disables the row in a browser.
addAction({
  id: "workbench.action.quit",
  title: "Exit",
  f1: true,
  precondition: "!isWeb",
  menu: [{ id: MenuId.MenubarFileMenu, group: "z_Exit", order: 1 }],
  run: async () => {
    await invoke("quit");
  },
});
