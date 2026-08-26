// The layout lock: one boolean shared by the Window menu and the lock
// control on every zone header. While locked, Dockview refuses drags and
// style.css hides the tab strip and drop affordances behind the
// .dock--locked class; unlocking reveals them. The lock freezes user
// rearrangement only - openInZone and fromJSON place panels the same in
// either state.

import type {
  DockviewApi,
  DockviewGroupPanel,
  IGroupHeaderProps,
  IHeaderActionsRenderer,
} from "dockview";

let dock: DockviewApi | null = null;
let dockElement: HTMLElement | null = null;
let locked = true;
const listeners = new Set<(locked: boolean) => void>();

function apply(): void {
  dock?.updateOptions({ locked });
  dockElement?.classList.toggle("dock--locked", locked);
}

/** Binds the lock to the dock and applies the initial (locked) state. */
export function initLayoutLock(dockview: DockviewApi, element: HTMLElement): void {
  dock = dockview;
  dockElement = element;
  apply();
}

/** The current lock state; the layout boots locked. */
export function isLayoutLocked(): boolean {
  return locked;
}

/**
 * Sets the lock. The Window menu and every zone-header lock control call
 * here, so the two surfaces can never disagree.
 */
export function setLayoutLocked(next: boolean): void {
  if (next === locked) {
    return;
  }
  locked = next;
  apply();
  for (const listener of listeners) {
    listener(locked);
  }
}

/** Subscribes to lock changes; returns an unsubscribe. */
export function onLayoutLockChange(listener: (locked: boolean) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Thin-stroke padlock glyphs matching the title-bar control style.
const LOCKED_SVG =
  '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1" aria-hidden="true"><rect x="2.5" y="5.5" width="7" height="5"/><path d="M4 5.5v-2a2 2 0 0 1 4 0v2"/></svg>';
const UNLOCKED_SVG =
  '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1" aria-hidden="true"><rect x="2.5" y="5.5" width="7" height="5"/><path d="M4 5.5v-2a2 2 0 0 1 3.5-1.6"/></svg>';

/**
 * The lock control on each zone header, rendered into the header's right
 * actions by Dockview. The button mirrors the shared lock state and
 * toggles it on click; while the layout is locked it is the only header
 * affordance style.css leaves visible.
 */
export function createLockHeaderControl(_group: DockviewGroupPanel): IHeaderActionsRenderer {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "layout-lock-toggle";
  const render = (state: boolean): void => {
    button.innerHTML = state ? LOCKED_SVG : UNLOCKED_SVG;
    // The label names the action, so no aria-pressed: a toggle button
    // announces state one way, never both.
    const label = state ? "Unlock layout" : "Lock layout";
    button.setAttribute("aria-label", label);
    button.title = label;
  };
  button.addEventListener("click", () => {
    setLayoutLocked(!isLayoutLocked());
  });
  const unsubscribe = onLayoutLockChange(render);
  return {
    element: button,
    init(_params: IGroupHeaderProps): void {
      render(locked);
    },
    dispose(): void {
      unsubscribe();
    },
  };
}
