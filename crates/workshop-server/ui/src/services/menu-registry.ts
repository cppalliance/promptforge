// The menu registry: a Map of menu rows per menu id (the VS Code
// MenuRegistry pattern). A row is either a reference to a command in
// the command registry or a submenu whose payload is another menu id;
// the menubar is the root menu's submenu rows. Menus whose rows are
// dynamic (Open Recent mirrors the recent-files store) register a
// provider instead of static rows, and the widget re-reads it at every
// open. Separators are group boundaries; there is no separator row
// kind.
//
// The module-level Menus instance is the shared registry the
// composition root and feature contribution files populate; tests
// construct their own.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { toDisposable, type IDisposable } from "../base/lifecycle";

/**
 * The well-known menu ids. Features add their own ids - the type is
 * plain string - so this object names only the ids the workbench
 * itself contributes to.
 */
export const MenuId = {
  MenubarMainMenu: "menubar",
  MenubarFileMenu: "menubar/file",
  MenubarEditMenu: "menubar/edit",
  MenubarSelectionMenu: "menubar/selection",
  MenubarViewMenu: "menubar/view",
  MenubarGoMenu: "menubar/go",
  MenubarRunMenu: "menubar/run",
  MenubarTerminalMenu: "menubar/terminal",
  MenubarHelpMenu: "menubar/help",
  MenubarRecentMenu: "menubar/file/recent",
  CommandPalette: "commandPalette",
  CommandCenter: "commandCenter",
} as const;

/** A menu id. Plain string so features add their own. */
export type MenuId = string;

/** A menu row that dispatches a command from the command registry. */
export interface MenuItem {
  /** The command id the row dispatches. */
  readonly command: string;
  /** The arguments passed to the command's run. */
  readonly args?: readonly unknown[];
  /** The row label; defaults to the command's title. */
  readonly title?: string;
  /** The when-expression gating visibility. */
  readonly when?: string;
  /** The sort group; "navigation" sorts first. */
  readonly group?: string;
  /** The position within the group. */
  readonly order?: number;
}

/** A menu row that opens another menu. */
export interface SubmenuItem {
  /** The menu id this row opens. */
  readonly submenu: MenuId;
  /** The row label. */
  readonly title: string;
  /** The when-expression gating visibility. */
  readonly when?: string;
  /** The sort group; "navigation" sorts first. */
  readonly group?: string;
  /** The position within the group. */
  readonly order?: number;
}

/** One menu row: a command reference or a submenu. */
export type MenuRow = MenuItem | SubmenuItem;

/** The dynamic row source for a menu, re-read at every open. */
export type MenuItemsProvider = () => readonly MenuItem[];

/** The row's upsert identity: the command id or the submenu id. */
function rowId(row: MenuRow): string {
  return "submenu" in row ? row.submenu : row.command;
}

/** The row's sort title: its label, else the command id. */
function rowTitle(row: MenuRow): string {
  return row.title ?? ("submenu" in row ? row.submenu : row.command);
}

/**
 * Sorts rows into render order: the navigation group first, then
 * groups lexically, then order, then title. A row without a group
 * joins navigation, matching VS Code's implicit default. Group
 * clusters come out contiguous, so the widget draws separators on the
 * boundaries.
 */
function compareRows(a: MenuRow, b: MenuRow): number {
  const groupA = a.group ?? "navigation";
  const groupB = b.group ?? "navigation";
  const navA = groupA === "navigation" ? 0 : 1;
  const navB = groupB === "navigation" ? 0 : 1;
  if (navA !== navB) {
    return navA - navB;
  }
  if (groupA !== groupB) {
    return groupA < groupB ? -1 : 1;
  }
  const order = (a.order ?? 0) - (b.order ?? 0);
  if (order !== 0) {
    return order;
  }
  const titleA = rowTitle(a);
  const titleB = rowTitle(b);
  if (titleA !== titleB) {
    return titleA < titleB ? -1 : 1;
  }
  return 0;
}

export class MenuRegistry {
  private readonly items = new Map<string, MenuRow[]>();
  private readonly providers = new Map<string, MenuItemsProvider>();

  /**
   * Places a row in a menu. The row's identity is its command id or
   * submenu id: re-appending it upserts in place, so re-running a
   * setup never duplicates rows. The returned disposable removes only
   * this registration, so disposing a stale row cannot evict its
   * replacement.
   */
  appendMenuItem(menuId: MenuId, item: MenuItem | SubmenuItem): IDisposable {
    let list = this.items.get(menuId);
    if (list === undefined) {
      list = [];
      this.items.set(menuId, list);
    }
    const existing = list.findIndex((row) => rowId(row) === rowId(item));
    if (existing === -1) {
      list.push(item);
    } else {
      list[existing] = item;
    }
    return toDisposable(() => {
      const current = this.items.get(menuId);
      const index = current?.indexOf(item) ?? -1;
      if (current !== undefined && index !== -1) {
        current.splice(index, 1);
      }
    });
  }

  /** Registers the dynamic row source for a menu; replaces any previous. */
  setProvider(menuId: MenuId, provider: MenuItemsProvider): IDisposable {
    this.providers.set(menuId, provider);
    return toDisposable(() => {
      if (this.providers.get(menuId) === provider) {
        this.providers.delete(menuId);
      }
    });
  }

  /**
   * The menu's rows, static and provider rows sorted together:
   * navigation first, then groups lexically, then order, then title.
   */
  getMenuItems(menuId: MenuId): readonly MenuRow[] {
    const statics = this.items.get(menuId) ?? [];
    const dynamic = this.providers.get(menuId)?.() ?? [];
    return [...statics, ...dynamic].sort(compareRows);
  }
}

/** The shared registry the running app renders menus from. */
export const Menus = new MenuRegistry();

/** Places a row in a menu of the shared registry. */
export function appendMenuItem(menuId: MenuId, item: MenuItem | SubmenuItem): IDisposable {
  return Menus.appendMenuItem(menuId, item);
}
