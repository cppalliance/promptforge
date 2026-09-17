// The Workshop file tree panel. Lists the granted workspace roots and
// browses one directory at a time through GET /workspace/tree; activating
// a file opens it in the editor zone through openInZone. Browsing is
// paths only - the tree never reads file contents. Listings arrive
// through the validated workspace-api boundary. Expansion state and
// fetched listings live in the TreeStateService (resolved through the
// service registry), so closing and reopening the Workshop panel restores
// the tree as the user left it; the expansion also comes back from the
// workspace file on relaunch, without listings, so a folder that renders
// expanded with nothing cached fetches its listing then and there. The
// roots themselves come from the service's shared roots() load, which
// the window title reads too, so a boot or a workspace change fetches
// them once. A wholesale replacement of the expanded set (a workspace
// switch) fires the service's change event and the panel re-renders.
// The panel also manages the grants themselves: a root row's context
// menu revokes it, and a header "+" button (or the empty-space context
// menu) adds a folder - through the native folder picker in the desktop
// app, through a typed-path dialog in a plain browser.

import type { GroupPanelPartInitParameters } from "dockview";

import { toDisposable } from "../../base/lifecycle";
import { WorkshopPart } from "../../base/workshop-part";
import { DOCK, resolvePanelContent } from "../../services/panel-registry";
import { getService } from "../../services/service-registry";
import { ROOTS_KEY, TREE_STATE, type TreeStateService } from "../../services/tree-state-service";
import { fetchTree, revokeRoot, type TreeEntry, type TreeListing } from "../../services/workspace-api";
import { addFolderToWorkspace } from "../workspace/add-folder";
import { WORKSPACE_CHANGED_EVENT } from "../workspace/workspace-drops";
import { DropdownMenu } from "shared-ui/dropdown";
import { ICON_FOLDER_PLUS, ICON_TRASH_2 } from "../shared/icons";
import { openInZone, panelIdFor } from "./zones";

/** The status-bar surface the panel paints action outcomes onto. */
export interface TreeStatusSink {
  showLocal(label: string, severity: "info" | "error"): void;
}

const CHEVRON_SVG =
  '<svg class="ws-workshop-tree__chevron" width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 1.5l3.5 3.5-3.5 3.5" /></svg>';

export class WorkshopTreePanel extends WorkshopPart {
  private readonly list = document.createElement("ul");
  // The panel's context menus, at most one open at a time.
  private readonly dropdown = this._register(new DropdownMenu());
  // The current menu's 0x0 fixed-position anchor under the cursor, so
  // the dropdown can anchor a context menu at the pointer.
  private pointerAnchor: HTMLElement | null = null;
  // The open Add Folder dialog, dismissed with the panel.
  private dialog: { dispose(): void } | null = null;
  // A dropped folder grants a new root after this panel rendered; the
  // change event refetches the roots so the drop is visible immediately.
  private readonly onWorkspaceChanged = (): void => {
    this.state.invalidateRoots();
    this.reload();
  };

  constructor(private readonly statusBar: TreeStatusSink | null = null) {
    super();
    this.element.className = "ws-workshop-tree";
    // Focusable so Ctrl+Shift+F can land on the tree even while it is empty.
    this.element.tabIndex = -1;
    this.list.className = "ws-workshop-tree__list";
  }

  /** The session state: expansion and fetched listings survive a reopen. */
  private get state(): TreeStateService {
    return getService(TREE_STATE);
  }

  override init(parameters: GroupPanelPartInitParameters): void {
    super.init(parameters);
    window.addEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged);
    this._register(
      toDisposable(() => window.removeEventListener(WORKSPACE_CHANGED_EVENT, this.onWorkspaceChanged)),
    );
    // A replaced expanded set (Open Workspace from File) re-renders from
    // the cache; folders newly expanded with no listing fetch on render.
    this._register(this.state.onDidChange(() => this.reload()));
    void this.loadRoots().catch((error: unknown) => {
      this.showRootsError(error);
    });
  }

  protected create(parent: HTMLElement): void {
    parent.appendChild(this.buildHeader());
    parent.appendChild(this.list);
    // Right-clicking the panel's empty space offers Add Folder; root rows
    // stop propagation, and other rows fall through to the browser menu.
    parent.addEventListener("contextmenu", (event) => {
      const target = event.target;
      if (target instanceof Element && target.closest(".ws-workshop-tree__row") !== null) {
        return;
      }
      event.preventDefault();
      this.dropdown.show(this.menuAnchor(event, this.element), [
        {
          label: "Add Folder to Workspace...",
          iconHtml: ICON_FOLDER_PLUS,
          onClick: () => {
            this.addFolder();
          },
        },
      ]);
    });
  }

  override dispose(): void {
    this.dialog?.dispose();
    this.dialog = null;
    this.pointerAnchor?.remove();
    this.pointerAnchor = null;
    super.dispose();
  }

  /** The panel header: an icon button that starts the Add Folder flow. */
  private buildHeader(): HTMLElement {
    const header = document.createElement("div");
    header.className = "ws-workshop-tree__header";
    const add = document.createElement("button");
    add.type = "button";
    add.className = "ws-workshop-tree__add";
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
    this.clearRoots();
    void this.loadRoots().catch((error: unknown) => {
      this.showRootsError(error);
    });
  }

  /** Focuses the first tree row, or the tree itself while it is empty. */
  focus(): void {
    const row = this.element.querySelector<HTMLElement>(".ws-workshop-tree__row");
    (row ?? this.element).focus();
  }

  /**
   * Renders the granted roots from the service's shared load (cached,
   * in flight, or started here). Two loads can overlap - a workspace
   * change and a replaceExpanded a round-trip later, with the cache
   * empty - and both settle on the same listing, so each paints the
   * whole list afresh rather than appending: one copy of each root
   * however many loads settle. A listing the service did not cache is
   * stale (invalidateRoots ran while it was in flight) and is dropped;
   * so is a failure that a later load has since superseded with a
   * cached listing.
   */
  private async loadRoots(): Promise<void> {
    let listing: TreeListing;
    try {
      listing = await this.state.roots();
    } catch (error) {
      if (this.state.listing(ROOTS_KEY) !== undefined) {
        return;
      }
      throw error;
    }
    if (this.state.listing(ROOTS_KEY) !== listing) {
      return;
    }
    this.clearRoots();
    this.renderListing(this.list, listing, true);
    if (listing.entries.length === 0) {
      const empty = document.createElement("p");
      empty.className = "ws-workshop-tree__empty";
      empty.textContent = "Drop a folder onto the window to browse it here.";
      this.element.appendChild(empty);
    }
  }

  /** Empties the roots list and removes the empty hint. */
  private clearRoots(): void {
    this.list.textContent = "";
    this.element.querySelector(".ws-workshop-tree__empty")?.remove();
  }

  /**
   * Paints a roots load failure as the list's only row: overlapping loads
   * that failed together report once, not once per load.
   */
  private showRootsError(error: unknown): void {
    this.clearRoots();
    this.showError(this.list, error);
  }

  /** Appends one row per entry; the server orders directories first. */
  private renderListing(list: HTMLUListElement, listing: TreeListing, roots = false): void {
    for (const entry of listing.entries) {
      list.appendChild(this.renderEntry(entry, roots));
    }
  }

  private renderEntry(entry: TreeEntry, isRoot = false): HTMLLIElement {
    const item = document.createElement("li");
    item.className = "ws-workshop-tree__item";
    const row = document.createElement("button");
    row.type = "button";
    row.className = `ws-workshop-tree__row ws-workshop-tree__row--${entry.kind}`;
    row.title = entry.path;
    const name = document.createElement("span");
    name.className = "ws-workshop-tree__name";
    name.textContent = entry.name;
    if (isRoot) {
      row.addEventListener("contextmenu", (event) => {
        event.preventDefault();
        event.stopPropagation();
        this.dropdown.show(this.menuAnchor(event, row), [
          {
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
        row.classList.add("ws-workshop-tree__row--missing");
        const missing = document.createElement("span");
        missing.className = "ws-workshop-tree__missing";
        missing.textContent = "missing";
        row.appendChild(missing);
      }
      const expanded = this.state.isExpanded(entry.path);
      row.setAttribute("aria-expanded", String(expanded));
      const children = document.createElement("ul");
      children.className = "ws-workshop-tree__children";
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
      // An interactively expanded directory always has a cached listing:
      // expansion and caching happen together in toggle(). One restored
      // from the workspace file (or replaced on a workspace switch) has
      // none yet, so it fetches here and fills in when the listing lands.
      if (expanded) {
        const cached = this.state.listing(entry.path);
        if (cached !== undefined) {
          this.renderListing(children, cached);
        } else {
          void this.fillRestored(entry, row, children).catch((error: unknown) => {
            this.showError(children, error);
          });
        }
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
    if (this.state.isExpanded(entry.path)) {
      this.state.collapse(entry.path);
      children.hidden = true;
      row.setAttribute("aria-expanded", "false");
      return;
    }
    let listing = this.state.listing(entry.path);
    let fresh = false;
    if (listing === undefined) {
      row.disabled = true;
      try {
        listing = await fetchTree(entry.path);
        fresh = true;
      } finally {
        row.disabled = false;
      }
      this.state.cacheListing(entry.path, listing);
    }
    // A fresh fetch follows a failed attempt whose error row is still in
    // the list; clear before rendering. A cached listing re-renders only
    // into an empty list (collapse keeps the rows, just hidden).
    if (fresh || children.childElementCount === 0) {
      children.textContent = "";
      this.renderListing(children, listing);
    }
    this.state.expand(entry.path);
    children.hidden = false;
    row.setAttribute("aria-expanded", "true");
  }

  /**
   * Fetches and renders the listing of a directory that renders expanded
   * with nothing cached (a restored expansion). The row is disabled
   * while the fetch is in flight, as toggle() does, so a click cannot
   * race it into a second fetch; the children list is already visible.
   */
  private async fillRestored(
    entry: TreeEntry,
    row: HTMLButtonElement,
    children: HTMLUListElement,
  ): Promise<void> {
    row.disabled = true;
    let listing: TreeListing;
    try {
      listing = await fetchTree(entry.path);
    } finally {
      row.disabled = false;
    }
    this.state.cacheListing(entry.path, listing);
    // The user may have collapsed the folder, or a reload may have
    // replaced the rows, while the fetch was in flight; render only into
    // a still-attached, still-expanded, still-empty list.
    if (!row.isConnected || !this.state.isExpanded(entry.path) || children.childElementCount !== 0) {
      return;
    }
    this.renderListing(children, listing);
  }

  /** Paints a load failure as a row in the affected list. */
  private showError(list: HTMLUListElement, error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    const row = document.createElement("li");
    row.className = "ws-workshop-tree__error";
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
    // A fresh anchor per menu: reusing one element would make the
    // dropdown read the next right-click as a same-trigger toggle and
    // close the menu it should be opening.
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
   * Starts the Add Folder flow: the native picker in the desktop app, a
   * typed-path dialog in a plain browser. The flow lives in the
   * workspace feature's add-folder module, shared with the File menu's
   * Open Folder and Add Folder to Workspace rows; the panel owns the
   * dialog's dismissal with its own lifetime.
   */
  private addFolder(): void {
    this.dialog?.dispose();
    this.dialog = addFolderToWorkspace(this.element, this.statusBar);
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

/** Ctrl+B: toggle the Workshop tree panel; answers the new visibility. */
export function toggleWorkshopPanel(): boolean {
  const dock = getService(DOCK);
  const existing = dock.getPanel(panelIdFor("tree", {}));
  if (existing) {
    dock.removePanel(existing);
    return false;
  }
  openInZone("tree", {});
  return true;
}

/** Ctrl+Shift+F: open or activate the Workshop tree and focus it. */
export function focusWorkshopTree(): void {
  const panel = openInZone("tree", {});
  // view.content may be the lazy wrapper while the chunk loads; unwrap
  // to the real panel before the instanceof check.
  const content = resolvePanelContent(panel.view.content);
  if (content instanceof WorkshopTreePanel) {
    content.focus();
  }
}
