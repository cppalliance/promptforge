// The menu popover widget: one self-sufficient popover that renders a
// menu id from the registries and owns its own dismissal (Escape,
// outside pointer, window blur). It takes only a MenuId, an anchor (an
// element or a point), and an optional context value passed as the first
// run argument to every row, so the menubar, and later a context menu at
// a pointer position, drive the same widget with no changes here.
//
// Rows are rebuilt from getMenuItems at every open: command rows versus
// submenu rows, group boundaries render separators (there is no
// separator row kind), and empty submenus are dropped. Per row: the
// label is the item title else the command title, the shortcut comes
// from the keybinding registry, a failing when hides the row, a failing
// precondition renders aria-disabled, and a toggled expression renders
// role=menuitemcheckbox with aria-checked. A submenu row owns the single
// child-flyout slot - one child Menu at a time, opened on hover or
// ArrowRight, closed on ArrowLeft, recursive for nested flyouts.
//
// While open the widget subscribes to context-key changes and rebuilds
// only when a key its rows reference changes, restoring focus by row key
// so a rebuild mid-navigation never strands a keyboard user.

import "./window-menu.css";

import { Emitter } from "../../base/event";
import { Disposable, DisposableStore, toDisposable } from "../../base/lifecycle";
import { Commands, type CommandRegistry } from "../../services/command-registry";
import { CONTEXT_KEY_SERVICE, type ContextKeyService } from "../../services/context-key-service";
import { ContextKeyExpr } from "../../services/context-key-expr";
import { KeybindingsRegistry } from "../../services/keybinding-registry";
import { Menus, type MenuId, type MenuItem, type MenuRegistry, type MenuRow, type SubmenuItem } from "../../services/menu-registry";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { STATUS_BAR } from "../status/status-bar";

/** Where the popover opens: below an element, or at a pointer position. */
export type MenuAnchor = HTMLElement | { readonly x: number; readonly y: number };

/** Registry and service overrides; tests inject their own instances. */
export interface MenuDependencies {
  readonly menus?: MenuRegistry;
  readonly commands?: CommandRegistry;
  readonly contextKeys?: ContextKeyService;
  readonly keybindings?: KeybindingsRegistry;
}

/** One rendered row: its stable key, element, and source registry row. */
interface RowHandle {
  readonly key: string;
  readonly element: HTMLButtonElement;
  readonly row: MenuRow;
}

/**
 * A menu popover bound to the registries. open() shows the menu anchored
 * to an element or point; close() dismisses the popover and any flyout.
 * The widget owns every dismissal path - Escape, outside pointer, window
 * blur - so composing widgets (the menubar) never track them.
 */
export class Menu extends Disposable {
  private readonly closeEmitter = new Emitter<void>();
  /** Fires after the popover closes, however it was dismissed. */
  readonly onDidClose = this.closeEmitter.event;
  private readonly menus: MenuRegistry;
  private readonly commands: CommandRegistry;
  private readonly contextKeys: ContextKeyService;
  private readonly keybindings: KeybindingsRegistry;

  private popover: HTMLElement | null = null;
  private rows: RowHandle[] = [];
  private openMenuId: MenuId | null = null;
  private anchor: MenuAnchor | null = null;
  private context: unknown;
  private watchedKeys: ReadonlySet<string> = new Set();
  private openStore: DisposableStore | null = null;
  private child: Menu | null = null;
  private childRow: HTMLButtonElement | null = null;
  private parent: Menu | null = null;
  private parentRow: HTMLButtonElement | null = null;

  constructor(deps: MenuDependencies = {}) {
    super();
    this.menus = deps.menus ?? Menus;
    this.commands = deps.commands ?? Commands;
    this.contextKeys = deps.contextKeys ?? getService(CONTEXT_KEY_SERVICE);
    this.keybindings = deps.keybindings ?? KeybindingsRegistry;
    this._register(this.closeEmitter);
    this._register(toDisposable(() => this.close()));
    this._register(
      toDisposable(() => {
        this.popover?.remove();
        this.popover = null;
      }),
    );
  }

  /** True while the popover is shown. */
  get isOpen(): boolean {
    return this.openMenuId !== null;
  }

  /** True while a submenu flyout is open; the flyout holds the keyboard. */
  get hasOpenFlyout(): boolean {
    return this.child !== null;
  }

  /** True when focus sits on one of this menu's own submenu rows. */
  get focusIsOnSubmenuRow(): boolean {
    const handle = this.focusedHandle();
    return handle !== null && "submenu" in handle.row;
  }

  /** Moves focus to the first row, after a keyboard-driven open. */
  focusFirstRow(): void {
    this.focusRow(0);
  }

  /**
   * Shows the menu's popover anchored to `anchor`, rebuilding its rows
   * from the registry. `context` becomes the first run argument of every
   * command row. Opening while open replaces the current menu.
   */
  open(menuId: MenuId, anchor: MenuAnchor, context?: unknown): void {
    this.close();
    this.openMenuId = menuId;
    this.anchor = anchor;
    this.context = context;
    const popover = this.ensurePopover();
    this.rebuildRows();
    this.positionPopover(anchor);
    popover.hidden = false;

    const store = new DisposableStore();
    this.openStore = store;
    store.add(
      this.contextKeys.onDidChangeContext((change) => {
        if (change.affectsSome(this.watchedKeys)) {
          this.rebuildRows();
        }
      }),
    );
    const onKeydown = (event: KeyboardEvent): void => this.onKeydown(event);
    document.addEventListener("keydown", onKeydown);
    store.add(toDisposable(() => document.removeEventListener("keydown", onKeydown)));
    const onPointerDown = (event: Event): void => {
      const target = event.target;
      if (target instanceof Node && this.containsInChain(target)) {
        return;
      }
      this.close();
    };
    document.addEventListener("pointerdown", onPointerDown);
    store.add(toDisposable(() => document.removeEventListener("pointerdown", onPointerDown)));
    // A window losing focus (Alt+Tab, taskbar click) must not leave a
    // stale popover covering the returned window.
    const onWindowBlur = (): void => this.close();
    window.addEventListener("blur", onWindowBlur);
    store.add(toDisposable(() => window.removeEventListener("blur", onWindowBlur)));
  }

  /** Dismisses the popover and any open flyout. Safe when closed. */
  close(): void {
    if (this.openMenuId === null) {
      return;
    }
    this.closeChild();
    this.openStore?.dispose();
    this.openStore = null;
    this.openMenuId = null;
    if (this.popover !== null) {
      this.popover.hidden = true;
    }
    this.closeEmitter.fire();
  }

  private ensurePopover(): HTMLElement {
    if (this.popover === null) {
      const popover = document.createElement("div");
      popover.className = "ws-window-titlebar__popover";
      if (this.parentRow !== null) {
        // A flyout opens beside its parent row; the class carries the
        // alignment offset (window-menu.css).
        popover.classList.add("ws-window-titlebar__popover--flyout");
      }
      popover.setAttribute("role", "menu");
      popover.hidden = true;
      document.body.appendChild(popover);
      this.popover = popover;
    }
    return this.popover;
  }

  private positionPopover(anchor: MenuAnchor): void {
    const popover = this.ensurePopover();
    if (anchor instanceof HTMLElement) {
      const rect = anchor.getBoundingClientRect();
      if (this.parentRow !== null) {
        // A flyout opens beside its parent row.
        popover.style.left = `${rect.right}px`;
        popover.style.top = `${rect.top}px`;
      } else {
        popover.style.left = `${rect.left}px`;
        popover.style.top = `${rect.bottom}px`;
      }
    } else {
      popover.style.left = `${anchor.x}px`;
      popover.style.top = `${anchor.y}px`;
    }
  }

  /**
   * Rebuilds the popover's rows from the registry. Wiping the popover
   * destroys the focused row, so the focused row is remembered by its
   * stable key (the command or submenu id, never the index) and focus
   * lands on the equivalent new row - or the first row when it vanished.
   */
  private rebuildRows(): void {
    const menuId = this.openMenuId;
    if (menuId === null) {
      return;
    }
    const popover = this.ensurePopover();
    const focusedKey = this.rows.find((row) => row.element === document.activeElement)?.key;
    // The flyout's rows derive from the old row set; it reopens on the
    // next hover or ArrowRight.
    this.closeChild();
    popover.textContent = "";
    this.rows = [];
    const watched = new Set<string>();
    let lastGroup: string | null = null;
    for (const row of this.menus.getMenuItems(menuId)) {
      if (!this.matches(row.when, watched)) {
        continue;
      }
      if ("submenu" in row && !this.hasVisibleRows(row.submenu, new Set([menuId]), watched)) {
        continue;
      }
      const group = row.group ?? "navigation";
      if (lastGroup !== null && group !== lastGroup) {
        const separator = document.createElement("div");
        separator.className = "ws-window-titlebar__separator";
        separator.setAttribute("role", "separator");
        popover.appendChild(separator);
      }
      lastGroup = group;
      const handle = "submenu" in row ? this.buildSubmenuRow(row) : this.buildCommandRow(row, watched);
      this.rows.push(handle);
      popover.appendChild(handle.element);
    }
    this.watchedKeys = watched;
    if (focusedKey !== undefined) {
      const restored = this.rows.find((row) => row.key === focusedKey) ?? this.rows[0];
      restored?.element.focus();
    }
  }

  /**
   * True when the menu has at least one row that would render: a command
   * whose when passes, or a submenu with visible rows of its own. The
   * seen set breaks submenu cycles, which render as empty.
   */
  private hasVisibleRows(menuId: MenuId, seen: ReadonlySet<string>, watched: Set<string>): boolean {
    if (seen.has(menuId)) {
      return false;
    }
    const nextSeen = new Set(seen).add(menuId);
    for (const row of this.menus.getMenuItems(menuId)) {
      if (!this.matches(row.when, watched)) {
        continue;
      }
      if ("submenu" in row) {
        if (this.hasVisibleRows(row.submenu, nextSeen, watched)) {
          return true;
        }
      } else {
        return true;
      }
    }
    return false;
  }

  /**
   * Evaluates a when/precondition/toggled string against the context
   * keys, recording the referenced keys for change filtering. Undefined
   * always matches; a malformed string never matches - registration is
   * where malformed strings are reported, so the row hides or disables
   * rather than throwing at render.
   */
  private matches(text: string | undefined, watched: Set<string>): boolean {
    if (text === undefined) {
      return true;
    }
    const result = ContextKeyExpr.deserialize(text);
    if (!result.ok) {
      return false;
    }
    for (const key of result.value.keys()) {
      watched.add(key);
    }
    return result.value.evaluate((key) => this.contextKeys.getValue(key));
  }

  private buildCommandRow(item: MenuItem, watched: Set<string>): RowHandle {
    const action = this.commands.lookup(item.command);
    // A placement without its command is a registration-order bug;
    // render it disabled and labelled rather than dropping it silently.
    const enabled = action !== undefined && this.matches(action.precondition, watched);
    const element = document.createElement("button");
    element.type = "button";
    element.className = "ws-window-titlebar__item";
    element.dataset["menuRowKey"] = item.command;
    element.setAttribute("aria-disabled", enabled ? "false" : "true");
    if (action?.toggled !== undefined) {
      element.classList.add("ws-window-titlebar__item--checkable");
      element.setAttribute("role", "menuitemcheckbox");
      const checked = this.matches(action.toggled, watched);
      element.setAttribute("aria-checked", checked ? "true" : "false");
      const check = document.createElement("span");
      check.className = "ws-window-titlebar__item-check";
      check.setAttribute("aria-hidden", "true");
      check.textContent = checked ? "✓" : "";
      element.appendChild(check);
    } else {
      element.setAttribute("role", "menuitem");
    }
    const label = document.createElement("span");
    label.className = "ws-window-titlebar__item-label";
    label.textContent = item.title ?? action?.title ?? item.command;
    element.appendChild(label);
    const shortcut = this.keybindings.lookupKeybinding(item.command)?.getLabel();
    if (shortcut !== undefined) {
      const hint = document.createElement("span");
      hint.className = "ws-window-titlebar__shortcut";
      hint.textContent = shortcut;
      element.appendChild(hint);
    }
    const handle: RowHandle = { key: item.command, element, row: item };
    element.addEventListener("click", () => this.activateCommand(handle));
    element.addEventListener("pointerenter", () => this.closeChild());
    return handle;
  }

  private buildSubmenuRow(item: SubmenuItem): RowHandle {
    const element = document.createElement("button");
    element.type = "button";
    element.className = "ws-window-titlebar__item";
    element.dataset["menuRowKey"] = item.submenu;
    element.setAttribute("role", "menuitem");
    element.setAttribute("aria-haspopup", "menu");
    element.setAttribute("aria-expanded", "false");
    const label = document.createElement("span");
    label.className = "ws-window-titlebar__item-label";
    label.textContent = item.title;
    element.appendChild(label);
    const chevron = document.createElement("span");
    chevron.className = "ws-window-titlebar__chevron";
    chevron.setAttribute("aria-hidden", "true");
    chevron.textContent = "›";
    element.appendChild(chevron);
    const handle: RowHandle = { key: item.submenu, element, row: item };
    element.addEventListener("pointerenter", () => this.openChild(handle, false));
    element.addEventListener("click", () => this.openChild(handle, true));
    return handle;
  }

  private activateCommand(handle: RowHandle): void {
    const row = handle.row;
    if ("submenu" in row || handle.element.getAttribute("aria-disabled") === "true") {
      return;
    }
    const args: readonly unknown[] =
      this.context === undefined ? (row.args ?? []) : [this.context, ...(row.args ?? [])];
    // Close the whole chain first, so a command that opens a surface
    // never stacks it under a stale popover.
    this.closeRoot();
    void this.commands.execute(row.command, ...args).catch((error: unknown) => {
      const statusBar = getServiceOrNull(STATUS_BAR);
      if (statusBar === null) {
        // No composition root (a widget test): keep the failure loud.
        console.error(`menu command '${row.command}' failed`, error);
        return;
      }
      const message = error instanceof Error ? error.message : String(error);
      statusBar.showLocal(`Could not run '${row.command}': ${message}`, "error");
    });
  }

  private openChild(handle: RowHandle, focusFirst: boolean): void {
    const row = handle.row;
    if (!("submenu" in row)) {
      return;
    }
    if (this.child !== null && this.childRow === handle.element) {
      return;
    }
    this.closeChild();
    const child = new Menu({
      menus: this.menus,
      commands: this.commands,
      contextKeys: this.contextKeys,
      keybindings: this.keybindings,
    });
    child.parent = this;
    child.parentRow = handle.element;
    this.child = child;
    this.childRow = handle.element;
    handle.element.setAttribute("aria-expanded", "true");
    child.open(row.submenu, handle.element, this.context);
    if (focusFirst) {
      child.focusRow(0);
    }
  }

  private closeChild(): void {
    const child = this.child;
    this.child = null;
    if (child !== null) {
      child.parent = null;
      child.close();
      child.dispose();
    }
    this.childRow?.setAttribute("aria-expanded", "false");
    this.childRow = null;
  }

  /** Detaches a flyout that closed itself (Escape, ArrowLeft). */
  private releaseChild(child: Menu): void {
    if (this.child !== child) {
      return;
    }
    this.child = null;
    child.dispose();
    this.childRow?.setAttribute("aria-expanded", "false");
    this.childRow = null;
  }

  private closeRoot(): void {
    let root: Menu = this;
    while (root.parent !== null) {
      root = root.parent;
    }
    root.close();
  }

  /** Closes this popover, returning focus to the parent row or anchor. */
  private closeSelf(): void {
    const parent = this.parent;
    const returnFocus = this.parentRow ?? (this.anchor instanceof HTMLElement ? this.anchor : null);
    this.close();
    parent?.releaseChild(this);
    returnFocus?.focus();
  }

  private containsInChain(target: Node): boolean {
    if (this.popover?.contains(target) === true) {
      return true;
    }
    // The anchor is the menu's own chrome: pressing it toggles the menu
    // (the menubar's click handler), so it must not read as an outside
    // press that dismisses the popover first.
    if (this.anchor instanceof HTMLElement && this.anchor.contains(target)) {
      return true;
    }
    return this.child?.containsInChain(target) ?? false;
  }

  private focusedHandle(): RowHandle | null {
    return this.rows.find((row) => row.element === document.activeElement) ?? null;
  }

  private focusRow(index: number): void {
    this.rows[index]?.element.focus();
  }

  private moveFocus(delta: number): void {
    const count = this.rows.length;
    if (count === 0) {
      return;
    }
    const current = this.rows.findIndex((row) => row.element === document.activeElement);
    const next = current === -1 ? (delta > 0 ? 0 : count - 1) : (current + delta + count) % count;
    this.focusRow(next);
  }

  private onKeydown(event: KeyboardEvent): void {
    // While a flyout is open it holds the keyboard; this menu's turn
    // resumes when the flyout closes.
    if (this.child !== null) {
      return;
    }
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        this.closeSelf();
        break;
      case "ArrowDown":
        event.preventDefault();
        this.moveFocus(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        this.moveFocus(-1);
        break;
      case "ArrowRight": {
        const handle = this.focusedHandle();
        if (handle !== null && "submenu" in handle.row) {
          event.preventDefault();
          this.openChild(handle, true);
        }
        break;
      }
      case "ArrowLeft":
        if (this.parentRow !== null) {
          event.preventDefault();
          this.closeSelf();
        }
        break;
      case "Enter": {
        const handle = this.focusedHandle();
        if (handle !== null) {
          // preventDefault also suppresses the button's native click.
          event.preventDefault();
          if ("submenu" in handle.row) {
            this.openChild(handle, true);
          } else {
            this.activateCommand(handle);
          }
        }
        break;
      }
    }
  }
}
