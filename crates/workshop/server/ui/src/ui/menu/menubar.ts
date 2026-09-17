// The menubar: the title bar's top-level menu buttons, generated from
// the MenubarMainMenu submenu rows in registry sort order, plus the
// bar-level interaction - click toggles a menu, rollover switches while
// one is open, ArrowLeft/ArrowRight move between menus. The popovers
// belong to the Menu widget (menu.ts); the bar composes one Menu and
// never tracks dismissal, which the widget owns (Escape, outside
// pointer, window blur).
//
// data-menu carries the menu id's last segment, so the selectors the
// tests key on ("file", "edit", ...) survive the move from static markup
// to generated buttons. The shipped nav is empty; the menu feature's
// bootstrap (ui/menu/index.ts) fills it through this generator at boot.

import { Disposable, toDisposable } from "../../base/lifecycle";
import { MenuId, Menus, type SubmenuItem } from "../../services/menu-registry";
import { Menu, type MenuDependencies } from "./menu";

/** The menu id's last segment: "menubar/file" -> "file". */
function lastSegment(id: MenuId): string {
  return id.slice(id.lastIndexOf("/") + 1);
}

/**
 * Generates one menubar button per submenu row, in the order given.
 * Buttons are <button type="button"> with the shared menu class,
 * data-menu set to the menu id's last segment, and the collapsed
 * aria-haspopup state. The caller owns the returned buttons.
 */
export function appendMenubarButtons(
  nav: HTMLElement,
  items: readonly SubmenuItem[],
): readonly HTMLButtonElement[] {
  const buttons: HTMLButtonElement[] = [];
  for (const item of items) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ws-window-titlebar__menu";
    button.dataset["menu"] = lastSegment(item.submenu);
    button.setAttribute("aria-haspopup", "menu");
    button.setAttribute("aria-expanded", "false");
    button.textContent = item.title;
    nav.appendChild(button);
    buttons.push(button);
  }
  return buttons;
}

/** One generated button and the menu it opens. */
interface BarEntry {
  readonly button: HTMLButtonElement;
  readonly menuId: MenuId;
}

/**
 * The title-bar menubar. The buttons come from MenubarMainMenu's
 * submenu rows at construction time: contributions register at module
 * scope, before the bar mounts. One shared Menu widget shows the open
 * menu's popover anchored to its button; the bar tracks only which
 * button is open, for aria-expanded, rollover, and arrow-key stepping.
 */
export class Menubar extends Disposable {
  private readonly menu: Menu;
  private readonly entries: readonly BarEntry[];
  private openEntry: BarEntry | null = null;

  constructor(nav: HTMLElement, deps: MenuDependencies = {}) {
    super();
    const menus = deps.menus ?? Menus;
    const items = menus
      .getMenuItems(MenuId.MenubarMainMenu)
      .filter((row): row is SubmenuItem => "submenu" in row);
    const buttons = appendMenubarButtons(nav, items);
    this.entries = items.map((item, index) => {
      const button = buttons[index];
      if (button === undefined) {
        throw new Error("DOM Error: menubar button generation lost a row.");
      }
      return { button, menuId: item.submenu };
    });
    this.menu = this._register(new Menu(deps));
    this._register(
      toDisposable(() => {
        for (const button of buttons) {
          button.remove();
        }
      }),
    );
    this._register(
      this.menu.onDidClose(() => {
        this.openEntry?.button.setAttribute("aria-expanded", "false");
        this.openEntry = null;
      }),
    );
    for (const entry of this.entries) {
      const onClick = (): void => {
        if (this.openEntry === entry && this.menu.isOpen) {
          this.menu.close();
        } else {
          this.openMenu(entry, false);
        }
      };
      entry.button.addEventListener("click", onClick);
      this._register(toDisposable(() => entry.button.removeEventListener("click", onClick)));
      // Menubar rollover: while any menu is open, hovering another
      // button switches the open menu to it. With no menu open, hover
      // alone opens nothing.
      const onEnter = (): void => {
        if (this.menu.isOpen && this.openEntry !== entry) {
          this.openMenu(entry, false);
        }
      };
      entry.button.addEventListener("pointerenter", onEnter);
      this._register(toDisposable(() => entry.button.removeEventListener("pointerenter", onEnter)));
    }
    const onKeydown = (event: KeyboardEvent): void => this.onKeydown(event);
    document.addEventListener("keydown", onKeydown);
    this._register(toDisposable(() => document.removeEventListener("keydown", onKeydown)));
  }

  private openMenu(entry: BarEntry, focusFirst: boolean): void {
    if (this.openEntry === entry && this.menu.isOpen) {
      return;
    }
    // open() closes any current menu first, which fires onDidClose and
    // collapses the previous button before this one expands.
    this.menu.open(entry.menuId, entry.button);
    this.openEntry = entry;
    entry.button.setAttribute("aria-expanded", "true");
    if (focusFirst) {
      this.menu.focusFirstRow();
    }
  }

  /**
   * ArrowLeft/ArrowRight step between menus. The bar yields to the
   * widget whenever the key belongs to flyout navigation: while a
   * flyout is open it owns both arrows, and ArrowRight on a submenu row
   * opens the flyout instead of switching menus.
   */
  private onKeydown(event: KeyboardEvent): void {
    if (!this.menu.isOpen || this.openEntry === null) {
      return;
    }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
      return;
    }
    if (this.menu.hasOpenFlyout) {
      return;
    }
    if (event.key === "ArrowRight" && this.menu.focusIsOnSubmenuRow) {
      return;
    }
    event.preventDefault();
    const current = this.entries.indexOf(this.openEntry);
    const delta = event.key === "ArrowRight" ? 1 : -1;
    const next = this.entries[(current + delta + this.entries.length) % this.entries.length];
    if (next !== undefined) {
      this.openMenu(next, true);
    }
  }
}
