// The menubar contribution: the menu tree's skeleton,
// registered eagerly at module scope, before any service exists. The
// eight top-level rows on MenubarMainMenu - File, Edit, Selection,
// View, Go, Run, Terminal, Help - plus the fourteen nested submenu
// declarations: a top-level menu is a SubmenuItem on the root menu and
// a flyout is a SubmenuItem on another menu, so the whole tree is rows
// in the one registry and the menubar widget generates its buttons from
// the root's sort order.
//
// Only the skeleton lives here: every command row arrives with its
// action from the feature contributions (or the stub table). File > New
// Window with Profile's dynamic profile rows have no backing store -
// the workshop has no window profiles - so that flyout holds only its
// static New Profile... stub. Open Recent's dynamic rows come from the
// workspace contribution's provider, not from this table.
//
// Placements follow the spec's row order: the root menu's rows stay
// groupless (the navigation default) with explicit orders, and each
// nested row names its parent menu's group so the separators fall where
// the spec has them.

import { appendMenuItem, MenuId } from "../../services/menu-registry";

/** One submenu declaration: the parent menu, the flyout it opens, and the row's placement. */
interface SubmenuRow {
  /** The menu the row lands in. */
  readonly menu: MenuId;
  /** The menu id the row opens. */
  readonly submenu: MenuId;
  /** The row label. */
  readonly title: string;
  /** The sort group; the root menu's rows stay groupless. */
  readonly group?: string;
  /** The position within the group, matching the spec's row order. */
  readonly order?: number;
}

const submenuRows = [
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarFileMenu, title: "File", order: 1 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarEditMenu, title: "Edit", order: 2 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarSelectionMenu, title: "Selection", order: 3 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarViewMenu, title: "View", order: 4 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarGoMenu, title: "Go", order: 5 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarRunMenu, title: "Run", order: 6 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarTerminalMenu, title: "Terminal", order: 7 },
  { menu: MenuId.MenubarMainMenu, submenu: MenuId.MenubarHelpMenu, title: "Help", order: 8 },
  { menu: MenuId.MenubarFileMenu, submenu: "menubar/file/newWindowWithProfile", title: "New Window with Profile", group: "1_new", order: 4 },
  { menu: MenuId.MenubarFileMenu, submenu: MenuId.MenubarRecentMenu, title: "Open Recent", group: "2_open", order: 4 },
  { menu: MenuId.MenubarFileMenu, submenu: "menubar/file/share", title: "Share", group: "5_share", order: 1 },
  { menu: MenuId.MenubarFileMenu, submenu: "menubar/file/preferences", title: "Preferences", group: "5_share", order: 3 },
  { menu: "menubar/file/preferences", submenu: "menubar/file/preferences/themes", title: "Themes", group: "1_settings", order: 7 },
  { menu: MenuId.MenubarViewMenu, submenu: "menubar/view/appearance", title: "Appearance", group: "2_submenus", order: 1 },
  { menu: MenuId.MenubarViewMenu, submenu: "menubar/view/editorLayout", title: "Editor Layout", group: "2_submenus", order: 2 },
  { menu: "menubar/view/appearance", submenu: "menubar/view/appearance/panelPosition", title: "Panel Position", group: "3_panel_layout", order: 2 },
  { menu: "menubar/view/appearance", submenu: "menubar/view/appearance/alignPanel", title: "Align Panel", group: "3_panel_layout", order: 3 },
  { menu: "menubar/view/appearance", submenu: "menubar/view/appearance/tabBar", title: "Tab Bar", group: "3_panel_layout", order: 4 },
  { menu: "menubar/view/appearance", submenu: "menubar/view/appearance/editorActionsPosition", title: "Editor Actions Position", group: "3_panel_layout", order: 5 },
  { menu: MenuId.MenubarGoMenu, submenu: "menubar/go/switchEditor", title: "Switch Editor", group: "2_switch", order: 1 },
  { menu: MenuId.MenubarGoMenu, submenu: "menubar/go/switchGroup", title: "Switch Group", group: "2_switch", order: 2 },
  { menu: MenuId.MenubarRunMenu, submenu: "menubar/run/newBreakpoint", title: "New Breakpoint", group: "4_breakpoints", order: 2 },
] satisfies readonly SubmenuRow[];

for (const row of submenuRows) {
  appendMenuItem(row.menu, { submenu: row.submenu, title: row.title, group: row.group, order: row.order });
}
