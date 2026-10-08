// The command center: a small toolbar over MenuId.CommandCenter, mounted
// inside the title bar's center drag region with the no-drag marker
// (window-chrome.ts starts a native drag only when the pointer lands on
// the region itself, so any child element is already exempt; the class
// documents the intent and is the styling hook). The toolbar's
// built-in item is the folder-name button: the window title as a quiet
// label, with no search icon and no ? chevron. A mouse press on it
// dispatches the menu's first command row
// (workbench.action.quickOpenWithModes, registered by the quickinput
// contribution) on mousedown, the way Cursor's title bar opens quick
// open; a click with no preceding press (detail 0, the keyboard's Enter
// or Space) does the same, and the click that ends a mouse press is
// ignored so one gesture never opens it twice. Any further command rows
// render as plain toolbar buttons beside it. Neither names the quick
// input widget; both dispatch through the command registry like every
// other surface.
//
// The WindowTitle helper owns the title text: the first granted root's
// folder name, or "PromptForge" when no root is granted or the listing
// cannot be read. It reads the roots through the tree-state service's
// shared roots() load - the same load the tree panel renders from, so a
// boot costs one GET /workspace/tree - re-reads them on the
// workspace-changed event (fired by workspace-drops.ts and the tree
// panel's Add/Remove Folder) and mirrors the text into document.title.

import "./command-center.css";

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import { CommandRegistry, Commands } from "@workshop/platform/command-registry";
import { MenuId, Menus, type MenuItem, type MenuRegistry } from "@workshop/platform/menu-registry";
import { getService, getServiceOrNull } from "@workshop/platform/service-registry";
import { TREE_STATE } from "../../services/tree-state-service";
import { STATUS_BAR } from "@workshop/platform/status-bar";
import { WORKSPACE_CHANGED_EVENT } from "../../services/workspace-events";

/** The title shown when no workspace folder is granted. */
const FALLBACK_TITLE = "PromptForge";

/** One granted root, narrowed to the field the title reads. */
interface GrantedRoot {
  readonly name: string;
}

/** Reads the granted roots; defaults to the tree-state service's shared load. */
export type ListRoots = () => Promise<readonly GrantedRoot[]>;

const defaultListRoots: ListRoots = async () => (await getService(TREE_STATE).roots()).entries;

/** Registry and roots overrides; tests inject their own. */
export interface CommandCenterDependencies {
  readonly commands?: CommandRegistry;
  readonly menus?: MenuRegistry;
  readonly listRoots?: ListRoots;
}

/**
 * The window title label. Shows the first granted root's folder name,
 * falling back to "PromptForge", and keeps document.title in step. A
 * failed listing renders the fallback rather than throwing at boot.
 */
export class WindowTitle extends Disposable {
  /** The label element; the command center appends it inside the folder-name button. */
  readonly element: HTMLSpanElement;

  private readonly listRoots: ListRoots;
  // A slow refetch must not overwrite a newer one: only the latest
  // generation may write the title.
  private generation = 0;

  constructor(deps: { readonly listRoots?: ListRoots } = {}) {
    super();
    this.listRoots = deps.listRoots ?? defaultListRoots;
    this.element = document.createElement("span");
    this.element.className = "ws-command-center__title";
    this.element.textContent = FALLBACK_TITLE;
    const onWorkspaceChanged = (): void => {
      void this.refresh();
    };
    window.addEventListener(WORKSPACE_CHANGED_EVENT, onWorkspaceChanged);
    this._register(toDisposable(() => window.removeEventListener(WORKSPACE_CHANGED_EVENT, onWorkspaceChanged)));
    void this.refresh();
  }

  private async refresh(): Promise<void> {
    const generation = ++this.generation;
    let title = FALLBACK_TITLE;
    try {
      const first = (await this.listRoots())[0];
      if (first !== undefined && first.name !== "") {
        title = first.name;
      }
    } catch {
      // The roots listing is unreadable (server down, bad shape): the
      // fallback title stands until the next workspace change.
    }
    if (generation !== this.generation) {
      return;
    }
    this.element.textContent = title;
    document.title = title;
  }
}

/**
 * The command center toolbar over MenuId.CommandCenter. Mounts into the
 * title bar's center drag region; the menu's first command row is the
 * folder-name button's dispatch, and any further command rows render as
 * toolbar buttons beside it.
 */
export class CommandCenter extends Disposable {
  private readonly commands: CommandRegistry;
  private readonly menus: MenuRegistry;

  constructor(center: HTMLElement, deps: CommandCenterDependencies = {}) {
    super();
    this.commands = deps.commands ?? Commands;
    this.menus = deps.menus ?? Menus;

    const container = document.createElement("div");
    container.className = "ws-command-center ws-window-titlebar__no-drag";

    const rows = this.menus
      .getMenuItems(MenuId.CommandCenter)
      .filter((row): row is MenuItem => !("submenu" in row));
    const folderRow = rows[0];

    // The tooltip, not an aria-label: the visible folder name is the
    // accessible name, and a label that dropped it would fail label-in-name.
    const folder = document.createElement("button");
    folder.type = "button";
    folder.className = "ws-command-center__folder";
    folder.title = "Search files, commands, and more";

    const title = this._register(new WindowTitle({ listRoots: deps.listRoots }));
    folder.appendChild(title.element);

    const open = (): void => {
      if (folderRow !== undefined) {
        this.run(folderRow.command, ...(folderRow.args ?? []));
      }
    };
    folder.addEventListener("mousedown", (event) => {
      if (event.button === 0) {
        // The press's default action moves focus to this button, and it
        // lands after open() has already focused the quick input's box;
        // cancelling it keeps the panel's keyboard focus.
        event.preventDefault();
        open();
      }
    });
    folder.addEventListener("click", (event) => {
      // A mouse press already opened quick open on mousedown and carries
      // a click count of 1 or more; only a keyboard-synthesized click
      // (Enter or Space, detail 0) still needs to open it.
      if (event.detail === 0) {
        open();
      }
    });

    container.appendChild(folder);
    for (const row of rows.slice(1)) {
      container.appendChild(this.renderRow(row));
    }
    center.appendChild(container);
    this._register(toDisposable(() => container.remove()));
  }

  /** Renders one extra menu row as a toolbar button beside the folder-name button. */
  private renderRow(row: MenuItem): HTMLButtonElement {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "ws-command-center__item";
    item.textContent = row.title ?? this.commands.lookup(row.command)?.title ?? row.command;
    item.addEventListener("click", () => this.run(row.command, ...(row.args ?? [])));
    return item;
  }

  /** Dispatches a command; a failure posts to the status bar. */
  private run(id: string, ...args: readonly unknown[]): void {
    void this.commands.execute(id, ...args).catch((error: unknown) => {
      const statusBar = getServiceOrNull(STATUS_BAR);
      if (statusBar === null) {
        // No composition root (a widget test): keep the failure loud.
        console.error(`command center command '${id}' failed`, error);
        return;
      }
      const message = error instanceof Error ? error.message : String(error);
      statusBar.showLocal(`Could not run '${id}': ${message}`, "error");
    });
  }
}
