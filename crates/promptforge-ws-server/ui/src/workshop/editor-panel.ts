// Placeholder editor panel: shows the workspace path it was opened with.
// Step 14 replaces the body with the CodeMirror 6 EditorSurface; the
// registry entry and the params contract ({ path }) are already final, so
// the real editor slots in without touching zones or the tree.

import type { GroupPanelPartInitParameters, IContentRenderer } from "dockview";

/** Reads the file path out of panel params, which arrive as unknown fields. */
function filePathParam(params: Record<string, unknown>): string | null {
  const path = params.path;
  return typeof path === "string" && path.length > 0 ? path : null;
}

export class EditorPanel implements IContentRenderer {
  readonly element = document.createElement("div");

  constructor() {
    this.element.className = "editor-panel";
  }

  init(parameters: GroupPanelPartInitParameters): void {
    const path = filePathParam(parameters.params);
    const label = document.createElement("p");
    label.className = "editor-panel__placeholder";
    label.textContent = path ?? "No file";
    this.element.appendChild(label);
  }
}
