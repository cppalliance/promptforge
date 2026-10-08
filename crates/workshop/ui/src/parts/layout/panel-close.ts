// The layout core's close path: Close and Close Others over any dock
// panel, each part confirming through confirmClose() and answering
// isDirty() for the unsaved checks. The generic tab's X and Delete call
// it directly; layout.contribution.ts registers the two commands over it.
// It resolves the dock through the service registry (the DOCK token,
// registered by initZones) instead of capturing it.
//
// Imports only @workshop/platform and type-only dockview: lazy panels
// import panel-tab.ts for setTabLoading, and panel-tab.ts imports this
// module, so a contribution, the parts/menu barrel or a feature module
// here would pull the entry bundle into every lazy chunk.

import type { IDockviewPanel } from "dockview";

import { Emitter, type Event } from "@workshop/platform/event";
import { DOCK, panelTypeEntry, resolvePanelContent } from "@workshop/platform/panel-registry";
import { getService } from "@workshop/platform/service-registry";
import { WorkshopPart } from "@workshop/platform/workshop-part";

// The panels this module closed, announced after each one is gone, so a
// feature can follow its own panels' closes (the agent contribution hides
// the chat pane when its last chat tab closes) without the close path
// naming any panel type. A close that does not go through this path - a
// layout restore, a workspace switch - announces nothing.
const closedEmitter = new Emitter<IDockviewPanel>();

/** Fires with each panel the close path closed, after it left the dock. */
export const onDidClosePanel: Event<IDockviewPanel> = closedEmitter.event;

/**
 * The panel a close command acts on: the one a `{ panelId }` argument
 * names, else the active panel. An argument naming no open panel targets
 * nothing.
 */
function closeTarget(arg: unknown): IDockviewPanel | undefined {
  const dock = getService(DOCK);
  if (typeof arg === "object" && arg !== null && "panelId" in arg) {
    return typeof arg.panelId === "string" ? dock.getPanel(arg.panelId) : undefined;
  }
  return dock.activePanel;
}

/** False for a panel whose type registered `closable: false`. */
function isClosable(panel: IDockviewPanel): boolean {
  return panelTypeEntry(panel.api.component)?.closable !== false;
}

/**
 * Whether the panel is still in the dock. A confirmation can outlive it,
 * and Dockview removes by id: closing a panel that already left throws,
 * or removes one reopened under its id.
 */
function isInDock(panel: IDockviewPanel): boolean {
  return getService(DOCK).getPanel(panel.id) === panel;
}

/** Whether the panel's part holds unsaved changes. */
function isUnsaved(panel: IDockviewPanel): boolean {
  // view.content may be the lazy wrapper while the chunk loads; unwrap
  // to the real part before the instanceof check.
  const content = resolvePanelContent(panel.view.content);
  return content instanceof WorkshopPart && content.isDirty();
}

/**
 * The panel part's confirmClose() answer. An unsaved part is activated
 * first: its prompt renders inside the panel, and Save As saves the
 * active panel. Content that is not a WorkshopPart needs no confirmation.
 */
function confirmPanelClose(panel: IDockviewPanel): Promise<boolean> {
  const content = resolvePanelContent(panel.view.content);
  if (!(content instanceof WorkshopPart)) {
    return Promise.resolve(true);
  }
  if (isUnsaved(panel)) {
    panel.api.setActive();
  }
  return content.confirmClose();
}

/**
 * Close (Ctrl+F4): closes the `{ panelId }` argument's panel, else the
 * active one, once its part confirms. A non-closable panel is left alone.
 * Resolves true only when it closed the panel.
 */
export async function closeActiveEditor(arg?: unknown): Promise<boolean> {
  const panel = closeTarget(arg);
  if (panel === undefined || !isClosable(panel)) {
    return false;
  }
  if ((await confirmPanelClose(panel)) && isInDock(panel)) {
    panel.api.close();
    closedEmitter.fire(panel);
    return true;
  }
  return false;
}

/**
 * Close Others: closes the other closable panels in the group of the
 * `{ panelId }` argument's panel, else the active one's. Every panel
 * confirms first, one at a time; the first refusal aborts the batch with
 * nothing closed, though saves and discards already made stand. A part
 * confirmed clean that gains unsaved changes while a later prompt is up
 * voids the batch too. Otherwise the whole batch closes, clean panels
 * included.
 */
export async function closeOtherEditors(arg?: unknown): Promise<void> {
  const target = closeTarget(arg);
  if (target === undefined) {
    return;
  }
  const batch = target.group.panels.filter((panel) => panel !== target && isClosable(panel));
  const confirmedClean: IDockviewPanel[] = [];
  for (const panel of batch) {
    if (!isInDock(panel)) {
      continue;
    }
    if (!(await confirmPanelClose(panel))) {
      return;
    }
    if (!isUnsaved(panel)) {
      confirmedClean.push(panel);
    }
  }
  if (confirmedClean.some((panel) => isInDock(panel) && isUnsaved(panel))) {
    return;
  }
  for (const panel of batch) {
    if (isInDock(panel)) {
      panel.api.close();
      closedEmitter.fire(panel);
    }
  }
}
