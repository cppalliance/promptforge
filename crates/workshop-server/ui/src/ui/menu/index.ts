// The menu feature's entry point and the workbench menu's bootstrap.
// setupWindowMenus imports the contribution surface - every action,
// menu row, keybinding rule, and quick-access provider registers into
// the shared registries at module scope - then starts the menubar on
// the title bar's shipped-empty nav. The bar generates its buttons from
// MenubarMainMenu's submenu rows in registry sort order, so the import
// must settle before the bar mounts; module evaluation order guarantees
// it. Tests and main.ts share this one entry point.

import "../workbench.contributions";

import type { IDisposable } from "../../base/lifecycle";
import { Menubar } from "./menubar";

/**
 * Starts the title-bar menubar over the shipped empty nav. Throws if
 * the markup is missing. The returned disposable releases the bar: the
 * generated buttons, the shared popover, and every listener.
 */
export function setupWindowMenus(): IDisposable {
  const nav = document.querySelector<HTMLElement>(".ws-window-titlebar__menus");
  if (!nav) {
    throw new Error("DOM Error: .ws-window-titlebar__menus not found in the page.");
  }
  return new Menubar(nav);
}
