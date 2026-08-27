// The Workshop file tree panel. Lists the granted workspace roots and
// browses one directory at a time through GET /workspace/tree; activating
// a file opens it in the editor zone through openInZone. Browsing is
// paths only - the tree never reads file contents. Listings arrive
// through the validated workspace-api boundary. Expansion state and
// fetched listings are kept for the running session, so closing and
// reopening the Workshop panel restores the tree as the user left it.

import type { IContentRenderer } from "dockview";

import { fetchTree, type TreeEntry, type TreeListing } from "../../services/workspace-api";
import { WORKSPACE_CHANGED_EVENT } from "../workspace-drops";
import { openInZone } from "./zones";

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
  // A dropped folder grants a new root after this panel rendered; the
  // change event refetches the roots so the drop is visible immediately.
  private readonly onWorkspaceChanged = (): void => {
    listingCache.delete(ROOTS_KEY);
    this.reload();
  };

  constructor() {
    this.element.className = "workshop-tree";
    // Focusable so Ctrl+Shift+F can land on the tree even while it is empty.
    this.element.tabIndex = -1;
    this.list.className = "workshop-tree__list";
  }

  init(): void {
    this.element.appendChild(this.list);
    window.addEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
    void this.loadRoots().catch((error: unknown) => {
      this.showError(this.list, error);
    });
  }

  dispose(): void {
    window.removeEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
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
    this.renderListing(this.list, listing);
    if (listing.entries.length === 0) {
      const empty = document.createElement("p");
      empty.className = "workshop-tree__empty";
      empty.textContent = "Drop a folder onto the window to browse it here.";
      this.element.appendChild(empty);
    }
  }

  /** Appends one row per entry; the server orders directories first. */
  private renderListing(list: HTMLUListElement, listing: TreeListing): void {
    for (const entry of listing.entries) {
      list.appendChild(this.renderEntry(entry));
    }
  }

  private renderEntry(entry: TreeEntry): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "workshop-tree__item";
    const row = document.createElement("button");
    row.type = "button";
    row.className = `workshop-tree__row workshop-tree__row--${entry.kind}`;
    row.title = entry.path;
    const name = document.createElement("span");
    name.className = "workshop-tree__name";
    name.textContent = entry.name;
    if (entry.kind === "directory") {
      row.insertAdjacentHTML("afterbegin", CHEVRON_SVG);
      row.appendChild(name);
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
}
