// The empty editor group's watermark: the product mark over a short list of
// shortcut rows, as Cursor shows in an editor area with nothing open. Dockview
// builds one per empty group through `createWatermarkComponent` (main.ts); the
// stylesheet shows it only in the main zone's groups, so an emptied side zone
// stays blank.
//
// The rows run New Agent, Show Files, Search Files, then Add Folder. A row
// names a command and reads its key caps from the keybinding registry, so a
// rebound chord shows as it is; a command with no keybinding gets no row,
// because a row with nothing to press has nothing to say. Add Folder is the
// Workshop's one conditional row: it shows only while exactly one root is
// granted, which the shared roots load answers (the tree panel and the window
// title read the same load, so the watermark costs no extra request). The
// count is re-read when the workspace changes; the tree-state service drops
// the shared load on that same event whether or not a tree panel is open
// (the composition root has it follow the event), which matters here because
// the empty editor's watermark is on screen exactly when Explorer is closed.
// The rows themselves hide in a group narrower than 478px (watermark.css),
// where they would not fit.

import "./watermark.css";

import type { IWatermarkRenderer, WatermarkRendererInitParameters } from "dockview";

import { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
import { getService } from "@workshop/platform/service-registry";
import { TREE_STATE } from "../../services/tree-state-service";
import { WORKSPACE_CHANGED_EVENT } from "../../services/workspace-events";

/** One watermark row: its label, the command whose keybinding it shows, and when it appears. */
export interface WatermarkRowSpec {
  readonly label: string;
  readonly commandId: string;
  /** Shows the row only while exactly this many roots are granted. */
  readonly whenRoots?: number;
}

/**
 * The rows, in order. Add Folder runs the Workshop's multi-root Add Folder
 * flow, which is what Open Folder does here, so it shows Open Folder's chord.
 */
export const WATERMARK_ROWS: readonly WatermarkRowSpec[] = [
  { label: "New Agent", commandId: "workbench.action.chat.new" },
  { label: "Show Files", commandId: "workbench.view.explorer" },
  { label: "Search Files", commandId: "workbench.action.quickOpen" },
  { label: "Add Folder", commandId: "workbench.action.files.openFolder", whenRoots: 1 },
];

/** The registry and the root count the watermark reads; tests inject their own. */
export interface WatermarkDeps {
  readonly keybindings?: Pick<KeybindingsRegistry, "lookupKeybinding">;
  /** How many roots are granted. Rejects when the listing cannot be read. */
  readonly rootCount?: () => Promise<number>;
}

const defaultRootCount = async (): Promise<number> => (await getService(TREE_STATE).roots()).entries.length;

/** The modifier names a formatted chord starts with, before its key. */
const MODIFIER_PREFIX = /^(?:(?:Ctrl|Shift|Alt|Meta|Cmd)\+)*/;

/**
 * Splits a formatted keybinding ("Ctrl+M Ctrl+O") into chords, each a list
 * of key names. The key is whatever follows the modifiers, so a "+" key
 * survives the split.
 */
export function keyCapsOf(label: string): string[][] {
  return label
    .split(" ")
    .filter((chord) => chord !== "")
    .map((chord) => {
      const modifiers = MODIFIER_PREFIX.exec(chord)?.[0] ?? "";
      const key = chord.slice(modifiers.length);
      return [...modifiers.split("+").filter((part) => part !== ""), key];
    });
}

class Watermark implements IWatermarkRenderer {
  readonly element = document.createElement("div");
  private readonly rows = document.createElement("div");
  private readonly keybindings: Pick<KeybindingsRegistry, "lookupKeybinding">;
  private readonly rootCount: () => Promise<number>;
  // The latest root count, or null while it is not known (before the first
  // read lands, or after one failed); a row that needs a count hides on null.
  private roots: number | null = null;
  // A slow read must not overwrite a newer one.
  private generation = 0;
  private disposed = false;

  private readonly onWorkspaceChanged = (): void => {
    // After every listener has run, so the tree-state service's invalidation
    // of the shared roots load lands before this read whatever the listener
    // order is.
    queueMicrotask(() => {
      if (!this.disposed) {
        void this.refresh();
      }
    });
  };

  constructor(deps: WatermarkDeps) {
    this.keybindings = deps.keybindings ?? KeybindingsRegistry;
    this.rootCount = deps.rootCount ?? defaultRootCount;
    this.element.className = "ws-watermark";
    const mark = document.createElement("img");
    mark.className = "ws-watermark__mark";
    mark.src = "/icons/promptforge-icon.png";
    mark.srcset = "/icons/promptforge-icon.png 1x, /icons/promptforge-icon@2x.png 2x";
    mark.alt = "";
    mark.draggable = false;
    this.rows.className = "ws-watermark__rows";
    this.element.append(mark, this.rows);
  }

  init(_parameters: WatermarkRendererInitParameters): void {
    this.render();
    window.addEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
    void this.refresh();
  }

  dispose(): void {
    this.disposed = true;
    window.removeEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
  }

  /** Re-reads the root count and repaints the rows. */
  private async refresh(): Promise<void> {
    const generation = ++this.generation;
    let count: number | null;
    try {
      count = await this.rootCount();
    } catch {
      // An unreadable listing leaves the count-gated row out until the next change.
      count = null;
    }
    if (this.disposed || generation !== this.generation) {
      return;
    }
    this.roots = count;
    this.render();
  }

  private render(): void {
    const rows: HTMLElement[] = [];
    for (const spec of WATERMARK_ROWS) {
      if (spec.whenRoots !== undefined && spec.whenRoots !== this.roots) {
        continue;
      }
      const label = this.keybindings.lookupKeybinding(spec.commandId)?.getLabel();
      if (label === undefined) {
        continue;
      }
      rows.push(this.buildRow(spec, label));
    }
    this.rows.replaceChildren(...rows);
  }

  private buildRow(spec: WatermarkRowSpec, keybinding: string): HTMLElement {
    const row = document.createElement("div");
    row.className = "ws-watermark__row";
    row.dataset["commandId"] = spec.commandId;
    const name = document.createElement("span");
    name.className = "ws-watermark__label";
    name.textContent = spec.label;
    const keys = document.createElement("span");
    keys.className = "ws-watermark__keys";
    for (const chord of keyCapsOf(keybinding)) {
      const group = document.createElement("span");
      group.className = "ws-watermark__chord";
      for (const key of chord) {
        const cap = document.createElement("kbd");
        cap.className = "ws-watermark__key";
        cap.textContent = key;
        group.append(cap);
      }
      keys.append(group);
    }
    row.append(name, keys);
    return row;
  }
}

/** Dockview's `createWatermarkComponent` seam: one watermark per empty group. */
export function createWatermark(deps: WatermarkDeps = {}): IWatermarkRenderer {
  return new Watermark(deps);
}
