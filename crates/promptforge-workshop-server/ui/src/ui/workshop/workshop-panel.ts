// The Workshop file tree panel. Lists the granted workspace roots and
// browses one directory at a time through GET /workspace/tree; activating
// a file opens it in the editor zone through openInZone. Browsing is
// paths only - the tree never reads file contents. Listings arrive
// through the validated workspace-api boundary. Expansion state and
// fetched listings are kept for the running session, so closing and
// reopening the Workshop panel restores the tree as the user left it.
// The panel also manages the grants themselves: a root row's context
// menu revokes it, and a header "+" button (or the empty-space context
// menu) adds a folder - through the native folder picker in the desktop
// app, through a typed-path dialog in a plain browser.

import type { IContentRenderer } from "dockview";
import { open } from "@tauri-apps/plugin-dialog";

import { showDropdown } from "../../chat/components/dropdown";
import { ICON_FOLDER_PLUS, ICON_TRASH_2 } from "../../chat/utils/icons";
import { fetchTree, revokeRoot, type TreeEntry, type TreeListing } from "../../services/workspace-api";
import { grantPath, WORKSPACE_CHANGED_EVENT } from "../workspace-drops";
import { showPanelDialog } from "./editor-dialog";
import { openInZone } from "./zones";

/** The status-bar surface the panel paints action outcomes onto. */
export interface TreeStatusSink {
  showLocal(label: string, severity: "info" | "error"): void;
}

// Cache key for the synthetic granted-roots listing, which has no path.
const ROOTS_KEY = "";

// Session state: expanded directory paths and the listings already
// fetched. Module-level so a reopened Workshop panel restores both.
const expandedPaths = new Set<string>();
const listingCache = new Map<string, TreeListing>();

const CHEVRON_SVG =
  '<svg class="workshop-tree__chevron" width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 1.5l3.5 3.5-3.5 3.5" /></svg>';

export class WorkshopTreePanel implements IContentRenderer {
  readonly element = document.createElement("div");
  private readonly list = document.createElement("ul");
  // The current menu's 0x0 fixed-position anchor under the cursor, so
  // the shared dropdown helper can anchor a context menu at the pointer.
  private pointerAnchor: HTMLElement | null = null;
  // The open Add Folder dialog, dismissed with the panel.
  private dialog: { dispose(): void } | null = null;
  // A dropped folder grants a new root after this panel rendered; the
  // change event refetches the roots so the drop is visible immediately.
  private readonly onWorkspaceChanged = (): void => {
    listingCache.delete(ROOTS_KEY);
    this.reload();
  };

  constructor(private readonly statusBar: TreeStatusSink | null = null) {
    this.element.className = "workshop-tree";
    // Focusable so Ctrl+Shift+F can land on the tree even while it is empty.
    this.element.tabIndex = -1;
    this.list.className = "workshop-tree__list";
  }

  init(): void {
    this.element.appendChild(this.buildHeader());
    this.element.appendChild(this.list);
    // Right-clicking the panel's empty space offers Add Folder; root rows
    // stop propagation, and other rows fall through to the browser menu.
    this.element.addEventListener("contextmenu", (event) => {
      const target = event.target;
      if (target instanceof Element && target.closest(".workshop-tree__row") !== null) {
        return;
      }
      event.preventDefault();
      showDropdown(this.menuAnchor(event, this.element), [
        {
          id: "workspace-add",
          label: "Add Folder to Workspace...",
          iconHtml: ICON_FOLDER_PLUS,
          onClick: () => {
            this.addFolder();
          },
        },
      ]);
    });
    window.addEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
    void this.loadRoots().catch((error: unknown) => {
      this.showError(this.list, error);
    });
  }

  dispose(): void {
    window.removeEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
    this.dialog?.dispose();
    this.dialog = null;
    this.pointerAnchor?.remove();
    this.pointerAnchor = null;
  }

  /** The panel header: an icon button that starts the Add Folder flow. */
  private buildHeader(): HTMLElement {
    const header = document.createElement("div");
    header.className = "workshop-tree__header";
    const add = document.createElement("button");
    add.type = "button";
    add.className = "workshop-tree__add";
    add.title = "Add Folder to Workspace...";
    add.setAttribute("aria-label", "Add Folder to Workspace");
    add.innerHTML = ICON_FOLDER_PLUS;
    add.addEventListener("click", () => {
      this.addFolder();
    });
    header.appendChild(add);
    return header;
  }

  /** Clears the rendered roots (and the empty hint) and renders afresh. */
  private reload(): void {
    this.list.textContent = "";
    this.element.querySelector(".workshop-tree__empty")?.remove();
    void this.loadRoots().catch((error: unknown) => {
      this.showError(this.list, error);
    });
  }

  /** Focuses the first tree row, or the tree itself while it is empty. */
  focus(): void {
    const row = this.element.querySelector<HTMLElement>(".workshop-tree__row");
    (row ?? this.element).focus();
  }

  /** Renders the granted roots, from the session cache when present. */
  private async loadRoots(): Promise<void> {
    let listing = listingCache.get(ROOTS_KEY);
    if (listing === undefined) {
      listing = await fetchTree(null);
      listingCache.set(ROOTS_KEY, listing);
    }
    this.renderListing(this.list, listing, true);
    if (listing.entries.length === 0) {
      const empty = document.createElement("p");
      empty.className = "workshop-tree__empty";
      empty.textContent = "Drop a folder onto the window to browse it here.";
      this.element.appendChild(empty);
    }
  }

  /** Appends one row per entry; the server orders directories first. */
  private renderListing(list: HTMLUListElement, listing: TreeListing, roots = false): void {
    for (const entry of listing.entries) {
      list.appendChild(this.renderEntry(entry, roots));
    }
  }

  private renderEntry(entry: TreeEntry, isRoot = false): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "workshop-tree__item";
    const row = document.createElement("button");
    row.type = "button";
    row.className = `workshop-tree__row workshop-tree__row--${entry.kind}`;
    row.title = entry.path;
    const name = document.createElement("span");
    name.className = "workshop-tree__name";
    name.textContent = entry.name;
    if (isRoot) {
      row.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
        showDropdown(this.menuAnchor(event, row), [
          {
            id: "workspace-remove",
            label: "Remove from Workspace",
            iconHtml: ICON_TRASH_2,
            danger: true,
            onClick: () => {
              void this.removeRoot(entry.path);
            },
          },
        ]);
      });
    }
    if (entry.kind === "directory") {
      row.insertAdjacentHTML("afterbegin", CHEVRON_SVG);
      row.appendChild(name);
      if (isRoot && !entry.exists) {
        // Not color alone: strikethrough (CSS), the danger color (CSS),
        // and this text label together mark a root deleted from disk.
        row.classList.add("workshop-tree__row--missing");
        const missing = document.createElement("span");
        missing.className = "workshop-tree__missing";
        missing.textContent = "missing";
        row.appendChild(missing);
      }
      const expanded = expandedPaths.has(entry.path);
      row.setAttribute("aria-expanded", String(expanded));
      const children = document.createElement("ul");
      children.className = "workshop-tree__children";
      children.hidden = !expanded;
      item.appendChild(row);
      item.appendChild(children);
      row.addEventListener("click", () => {
        void this.toggle(entry, row, children).catch((error: unknown) => {
          // The children list is hidden while collapsed; reveal it so the
          // error row is visible.
          children.hidden = false;
          this.showError(children, error);
        });
      });
      // An expanded directory always has a cached listing: expansion and
      // caching happen together in toggle().
      const cached = listingCache.get(entry.path);
      if (expanded && cached !== undefined) {
        this.renderListing(children, cached);
      }
    } else {
      row.appendChild(name);
      item.appendChild(row);
      row.addEventListener("click", () => {
        openInZone("editor", { path: entry.path });
      });
    }
    return item;
  }

  /** Expands a collapsed directory or collapses an expanded one. */
  private async toggle(
    entry: TreeEntry,
    row: HTMLButtonElement,
    children: HTMLUListElement,
  ): Promise<void> {
    if (expandedPaths.has(entry.path)) {
      expandedPaths.delete(entry.path);
      children.hidden = true;
      row.setAttribute("aria-expanded", "false");
      return;
    }
    let listing = listingCache.get(entry.path);
    let fresh = false;
    if (listing === undefined) {
      row.disabled = true;
      try {
        listing = await fetchTree(entry.path);
        fresh = true;
      } finally {
        row.disabled = false;
      }
      listingCache.set(entry.path, listing);
    }
    // A fresh fetch follows a failed attempt whose error row is still in
    // the list; clear before rendering. A cached listing re-renders only
    // into an empty list (collapse keeps the rows, just hidden).
    if (fresh || children.childElementCount === 0) {
      children.textContent = "";
      this.renderListing(children, listing);
    }
    expandedPaths.add(entry.path);
    children.hidden = false;
    row.setAttribute("aria-expanded", "true");
  }

  /** Paints a load failure as a row in the affected list. */
  private showError(list: HTMLUListElement, error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    const row = document.createElement("li");
    row.className = "workshop-tree__error";
    row.setAttribute("role", "alert");
    row.textContent = message;
    list.appendChild(row);
  }

  /**
   * The dropdown anchor for a context menu: the pointer position for a
   * mouse invocation, the row (or panel) itself for a keyboard one,
   * whose contextmenu event carries no coordinates.
   */
  private menuAnchor(event: MouseEvent, fallback: HTMLElement): HTMLElement {
    if (event.clientX === 0 && event.clientY === 0) {
      return fallback;
    }
    // A fresh anchor per menu: reusing one element would make the shared
    // dropdown helper read the next right-click as a same-trigger toggle
    // and close the menu it should be opening.
    this.pointerAnchor?.remove();
    const anchor = document.createElement("span");
    anchor.style.cssText =
      `position: fixed; width: 0; height: 0; pointer-events: none; ` +
      `left: ${event.clientX}px; top: ${event.clientY}px;`;
    document.body.appendChild(anchor);
    this.pointerAnchor = anchor;
    return anchor;
  }

  /**
   * Starts the Add Folder flow. In the desktop app the native folder
   * picker answers with the chosen path (a cancel answers nothing); in a
   * plain browser, where no picker and no OS paths exist, a dialog asks
   * for the path as text.
   */
  private addFolder(): void {
    if (window.__TAURI_INTERNALS__ !== undefined) {
      void this.pickFolder();
      return;
    }
    this.dialog?.dispose();
    this.dialog = showPanelDialog({
      host: this.element,
      classPrefix: "workspace-add",
      titleId: "workspace-add-title",
      title: "Add Folder to Workspace",
      message: "Enter the full path of a folder to browse in the Workshop.",
      field: { id: "workspace-add-path", label: "Folder path" },
      buttons: [
        {
          label: "Add",
          requiresValue: true,
          run: (value) => {
            void this.grantFolder(value);
          },
        },
        { label: "Cancel", run: () => undefined },
      ],
    });
  }

  /** The desktop pick: the native dialog; a cancel resolves null. */
  private async pickFolder(): Promise<void> {
    const picked = await open({ directory: true, title: "Add Folder to Workspace" });
    if (picked === null) {
      return;
    }
    await this.grantFolder(picked);
  }

  /** Grants one folder and announces the outcome, like the drop flow. */
  private async grantFolder(path: string): Promise<void> {
    try {
      await grantPath(path);
    } catch (error) {
      this.statusBar?.showLocal(`Could not add ${path}: ${(error as Error).message}`, "error");
      return;
    }
    this.statusBar?.showLocal(`Added ${path} to the Workshop`, "info");
    window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
  }

  /** Revokes one granted root and announces the outcome. */
  private async removeRoot(path: string): Promise<void> {
    try {
      await revokeRoot(path);
    } catch (error) {
      this.statusBar?.showLocal(`Could not remove ${path}: ${(error as Error).message}`, "error");
      return;
    }
    this.statusBar?.showLocal(`Removed ${path} from the Workshop`, "info");
    window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
  }
}
