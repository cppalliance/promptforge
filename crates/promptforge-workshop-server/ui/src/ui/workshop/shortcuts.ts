// App-level keyboard shortcuts: one document keydown listener dispatching
// to the workshop's command functions. The bindings are fixed - no
// customization, no chords. CodeMirror keeps typing, selection, clipboard,
// undo/redo, and in-file find/replace; these bindings cover
// workspace-level actions only: Ctrl+S save the active editor, Ctrl+W
// close it (prompting on unsaved changes), Ctrl+B toggle the Workshop
// tree, Ctrl+Tab / Ctrl+Shift+Tab cycle the editors, Ctrl+Shift+F focus
// the tree, Ctrl+= / Ctrl+- / Ctrl+0 zoom the window.

import type { DockviewApi, IDockviewPanel } from "dockview";

import { toDisposable, type IDisposable } from "../../base/lifecycle";
import { resetZoom, zoomIn, zoomOut } from "../zoom";
import { EditorPanel } from "./editor-panel";
import { WorkshopTreePanel } from "./workshop-panel";
import { openInZone, panelIdFor } from "./zones";

/** The panel's content as an EditorPanel, or null for other panel kinds. */
function asEditor(panel: IDockviewPanel | undefined): EditorPanel | null {
  if (panel === undefined) {
    return null;
  }
  const content = panel.view.content;
  return content instanceof EditorPanel ? content : null;
}

/** Every open editor panel, in dock order. */
function editorPanels(dock: DockviewApi): IDockviewPanel[] {
  return dock.panels.filter((panel) => asEditor(panel) !== null);
}

/** Ctrl+S: save the active editor. A no-op when no editor is active. */
export function saveActiveEditor(dock: DockviewApi): void {
  const editor = asEditor(dock.activePanel);
  if (editor !== null) {
    // save() handles its own failures (error bar, conflict dialog).
    void editor.save();
  }
}

/** Ctrl+W: close the active editor, prompting on unsaved changes. */
export function closeActiveEditor(dock: DockviewApi): void {
  asEditor(dock.activePanel)?.requestClose();
}

/** Ctrl+B: toggle the Workshop tree panel. */
export function toggleWorkshopPanel(dock: DockviewApi): void {
  const existing = dock.getPanel(panelIdFor("tree", {}));
  if (existing) {
    dock.removePanel(existing);
  } else {
    openInZone("tree", {});
  }
}

/** Ctrl+Tab / Ctrl+Shift+Tab: cycle the open editors, wrapping around. */
export function cycleEditor(dock: DockviewApi, direction: 1 | -1): void {
  const editors = editorPanels(dock);
  if (editors.length === 0) {
    return;
  }
  const current = editors.findIndex((panel) => panel === dock.activePanel);
  const index =
    current === -1
      ? direction === 1
        ? 0
        : editors.length - 1
      : (current + direction + editors.length) % editors.length;
  const panel = editors[index];
  if (panel === undefined) {
    return;
  }
  panel.api.setActive();
  asEditor(panel)?.focus();
}

/** Ctrl+Shift+F: open or activate the Workshop tree and focus it. */
export function focusWorkshopTree(): void {
  const panel = openInZone("tree", {});
  const content = panel.view.content;
  if (content instanceof WorkshopTreePanel) {
    content.focus();
  }
}

/** Only plain Ctrl combinations are bound; Alt and Meta stay untouched. */
function isPlainCtrl(event: KeyboardEvent): boolean {
  return event.ctrlKey && !event.altKey && !event.metaKey;
}

/**
 * Installs the app-level keydown listener. Unbound combinations fall
 * through without preventDefault so the browser and CodeMirror keep
 * theirs. Returns the disposable that uninstalls it.
 */
export function installShortcuts(dock: DockviewApi): IDisposable {
  const onKeydown = (event: KeyboardEvent): void => {
    if (!isPlainCtrl(event)) {
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      cycleEditor(dock, event.shiftKey ? -1 : 1);
      return;
    }
    // Zoom binds run before the Shift gate below: Ctrl+Shift+= reports
    // "+" for the same physical key Ctrl+= reports "=" for, and both are
    // the conventional zoom-in chord.
    if (event.code === "Equal" || event.key === "=" || event.key === "+") {
      event.preventDefault();
      zoomIn();
      return;
    }
    if (event.code === "Minus" || event.key === "-") {
      event.preventDefault();
      zoomOut();
      return;
    }
    if (event.code === "Digit0" || event.key === "0") {
      event.preventDefault();
      resetZoom();
      return;
    }
    const key = event.key.toLowerCase();
    if (event.shiftKey) {
      if (key === "f") {
        event.preventDefault();
        focusWorkshopTree();
      }
      return;
    }
    switch (key) {
      case "s":
        event.preventDefault();
        saveActiveEditor(dock);
        break;
      case "w":
        event.preventDefault();
        closeActiveEditor(dock);
        break;
      case "b":
        event.preventDefault();
        toggleWorkshopPanel(dock);
        break;
    }
  };
  document.addEventListener("keydown", onKeydown);
  return toDisposable(() => {
    document.removeEventListener("keydown", onKeydown);
  });
}
