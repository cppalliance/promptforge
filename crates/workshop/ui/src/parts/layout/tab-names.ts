// Tab names: the operator's own names for the tabs of renamable panel
// types (the chat tabs), kept for the session only. A rename never touches
// the panel's own title, which is what the layout persists and what a
// relaunch brings back, so a rename ends with the session by construction.
// The tab renderer shows the name in place of the title and listens for
// changes; the panel's close path forgets its name.
//
// Imports only @workshop/platform and the close path's event, so the
// lazy panel chunks that import the tab renderer stay light.

import { Emitter, type Event } from "@workshop/platform/event";
import { onDidClosePanel } from "./panel-close";

const names = new Map<string, string>();
const changeEmitter = new Emitter<string>();

/** Fires with the panel id whose tab name was set or cleared. */
export const onDidChangeTabName: Event<string> = changeEmitter.event;

/** The operator's name for a panel's tab, or undefined when the title stands. */
export function tabNameOf(panelId: string): string | undefined {
  return names.get(panelId);
}

/**
 * Names a panel's tab. The name is trimmed; a blank one clears the name
 * and the tab goes back to the panel's title.
 */
export function setTabName(panelId: string, name: string): void {
  const trimmed = name.trim();
  if (trimmed === "") {
    clearTabName(panelId);
    return;
  }
  if (names.get(panelId) === trimmed) {
    return;
  }
  names.set(panelId, trimmed);
  changeEmitter.fire(panelId);
}

/** Drops a panel's tab name, if it has one. */
export function clearTabName(panelId: string): void {
  if (names.delete(panelId)) {
    changeEmitter.fire(panelId);
  }
}

// A closed panel's name goes with it.
onDidClosePanel((panel) => clearTabName(panel.id));
