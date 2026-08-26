// Application menus for the custom title bar: accessible HTML popovers
// behind the File, Edit, Window, and Help buttons, not native menus. Every
// command dispatches through the single WindowMenuCommands set built here,
// so the popovers, future keyboard shortcuts, and any future menu surface
// all call the same actions. The Window menu reuses the visible window
// controls' command functions from window-chrome.ts.
//
// Edit commands go through document.execCommand: WebView2 hosts the page
// as application content with clipboard access, and execCommand preserves
// the editable target's native undo stack and selection semantics, which
// the async Clipboard API cannot. jsdom leaves execCommand undefined; the
// guard keeps the command a no-op there.

import { showAboutDialog } from "./about-dialog";
import type { ChatUI } from "./chat/main";
import { closeWindow, minimizeWindow, toggleWindowMaximize } from "./window-chrome";

/** The actions every menu surface and keyboard shortcut dispatches through. */
export interface WindowMenuCommands {
  readonly newChat: () => void;
  readonly closeWindow: () => void;
  readonly undo: () => void;
  readonly redo: () => void;
  readonly cut: () => void;
  readonly copy: () => void;
  readonly paste: () => void;
  readonly selectAll: () => void;
  readonly minimizeWindow: () => void;
  readonly toggleWindowMaximize: () => void;
  readonly showAbout: () => void;
}

/** The layout-lock command surface the Window menu toggles through. */
export interface LayoutLockCommands {
  readonly isLocked: () => boolean;
  readonly toggle: () => void;
}

const MENU_ORDER = ["file", "edit", "window", "help"] as const;
type MenuId = (typeof MENU_ORDER)[number];

const MENU_LABELS: Record<MenuId, string> = {
  file: "File",
  edit: "Edit",
  window: "Window",
  help: "Help",
};

interface CommandItem {
  readonly kind: "command";
  /** A string, or a function refreshed on every menu open (lock state). */
  readonly label: string | (() => string);
  readonly shortcut?: string;
  readonly run: () => void;
  readonly enabled?: () => boolean;
}

interface SeparatorItem {
  readonly kind: "separator";
}

type MenuItem = CommandItem | SeparatorItem;

interface CommandRow {
  readonly element: HTMLButtonElement;
  readonly labelElement: HTMLSpanElement;
  readonly def: CommandItem;
}

interface MenuHandle {
  readonly id: MenuId;
  readonly button: HTMLButtonElement;
  readonly popover: HTMLElement;
  readonly rows: readonly CommandRow[];
}

/** The editable elements the Edit menu commands act on. */
function isEditable(element: Element | null): element is HTMLElement {
  if (!(element instanceof HTMLElement)) {
    return false;
  }
  if (element instanceof HTMLTextAreaElement) {
    return !element.disabled && !element.readOnly;
  }
  if (element instanceof HTMLInputElement) {
    const textLike = ["text", "search", "url", "tel", "email", "password"];
    return !element.disabled && !element.readOnly && textLike.includes(element.type);
  }
  return element.isContentEditable;
}

function labelOf(def: CommandItem): string {
  return typeof def.label === "function" ? def.label() : def.label;
}

function buildMenuItems(
  commands: WindowMenuCommands,
  hasEditTarget: () => boolean,
  layoutLock?: LayoutLockCommands,
): Record<MenuId, readonly MenuItem[]> {
  const windowItems: MenuItem[] = [
    { kind: "command", label: "Minimize", run: commands.minimizeWindow },
    { kind: "command", label: "Maximize/Restore", run: commands.toggleWindowMaximize },
  ];
  if (layoutLock) {
    windowItems.push({ kind: "separator" });
    windowItems.push({
      kind: "command",
      label: () => (layoutLock.isLocked() ? "Unlock Layout" : "Lock Layout"),
      run: layoutLock.toggle,
    });
  }
  return {
    file: [
      { kind: "command", label: "New Chat", run: commands.newChat },
      { kind: "separator" },
      { kind: "command", label: "Close Window", shortcut: "Alt+F4", run: commands.closeWindow },
    ],
    edit: [
      { kind: "command", label: "Undo", shortcut: "Ctrl+Z", run: commands.undo, enabled: hasEditTarget },
      { kind: "command", label: "Redo", shortcut: "Ctrl+Y", run: commands.redo, enabled: hasEditTarget },
      { kind: "separator" },
      { kind: "command", label: "Cut", shortcut: "Ctrl+X", run: commands.cut, enabled: hasEditTarget },
      { kind: "command", label: "Copy", shortcut: "Ctrl+C", run: commands.copy, enabled: hasEditTarget },
      { kind: "command", label: "Paste", shortcut: "Ctrl+V", run: commands.paste, enabled: hasEditTarget },
      { kind: "separator" },
      { kind: "command", label: "Select All", shortcut: "Ctrl+A", run: commands.selectAll, enabled: hasEditTarget },
    ],
    window: windowItems,
    help: [{ kind: "command", label: "About PromptForge", run: commands.showAbout }],
  };
}

/**
 * Builds the shared command set and wires the title-bar menu buttons to
 * their popovers. The popovers only exist in the desktop shell, where the
 * bar is visible; in a plain browser the DOM wiring is skipped and only
 * the command set is returned. Throws if the title-bar markup is missing.
 */
export function setupWindowMenus(options: {
  readonly chat: ChatUI;
  readonly layoutLock?: LayoutLockCommands;
}): WindowMenuCommands {
  const navElement = document.querySelector<HTMLElement>(".window-titlebar__menus");
  if (!navElement) {
    throw new Error("DOM Error: .window-titlebar__menus not found in the page.");
  }
  // A separate const: narrowing does not propagate into the hoisted
  // function declarations below.
  const nav: HTMLElement = navElement;

  // Edit commands act on the editable element focused before the menu
  // opened. Clicking a menu button moves focus to the button, so the
  // target is remembered continuously instead of read at open time.
  let editTarget: HTMLElement | null = null;
  document.addEventListener("focusin", (event) => {
    const target = event.target instanceof Element ? event.target : null;
    if (isEditable(target)) {
      editTarget = target;
    }
  });
  const hasEditTarget = (): boolean => editTarget !== null && editTarget.isConnected;

  function runEditCommand(command: string): void {
    const target = editTarget;
    if (!target || !target.isConnected) {
      return;
    }
    target.focus();
    if (typeof document.execCommand === "function") {
      document.execCommand(command);
    }
  }

  const commands: WindowMenuCommands = {
    newChat: () => {
      void options.chat.engine.sessions.create().catch((error: unknown) => {
        console.error("New Chat failed:", error);
      });
    },
    closeWindow,
    undo: () => runEditCommand("undo"),
    redo: () => runEditCommand("redo"),
    cut: () => runEditCommand("cut"),
    copy: () => runEditCommand("copy"),
    paste: () => runEditCommand("paste"),
    selectAll: () => runEditCommand("selectAll"),
    minimizeWindow,
    toggleWindowMaximize,
    showAbout: showAboutDialog,
  };

  if (window.__PROMPTFORGE_DESKTOP__ !== true) {
    return commands;
  }

  const items = buildMenuItems(commands, hasEditTarget, options.layoutLock);
  const handles: MenuHandle[] = [];
  let openId: MenuId | null = null;

  function handleFor(id: MenuId): MenuHandle {
    const handle = handles.find((candidate) => candidate.id === id);
    if (!handle) {
      throw new Error(`DOM Error: the ${id} menu was not built.`);
    }
    return handle;
  }

  function refreshEnabled(handle: MenuHandle): void {
    for (const row of handle.rows) {
      const enabled = row.def.enabled ? row.def.enabled() : true;
      row.element.setAttribute("aria-disabled", enabled ? "false" : "true");
      row.labelElement.textContent = labelOf(row.def);
    }
  }

  function activateRow(row: CommandRow): void {
    if (row.element.getAttribute("aria-disabled") === "true") {
      return;
    }
    // Close first and return focus to the menu button, so a command that
    // opens a surface (the About dialog) records a visible invoker to
    // restore focus to on dismissal.
    const handle = openId === null ? null : handleFor(openId);
    closeMenu();
    handle?.button.focus();
    row.def.run();
  }

  function buildPopover(id: MenuId): MenuHandle {
    const button = nav.querySelector<HTMLButtonElement>(`[data-menu="${id}"]`);
    if (!button) {
      throw new Error(`DOM Error: the title bar is missing the ${MENU_LABELS[id]} menu button.`);
    }
    const popover = document.createElement("div");
    popover.className = "window-titlebar__popover";
    popover.setAttribute("role", "menu");
    popover.setAttribute("aria-label", MENU_LABELS[id]);
    popover.hidden = true;
    const rows: CommandRow[] = [];
    for (const def of items[id]) {
      if (def.kind === "separator") {
        const separator = document.createElement("div");
        separator.className = "window-titlebar__separator";
        separator.setAttribute("role", "separator");
        popover.appendChild(separator);
        continue;
      }
      const element = document.createElement("button");
      element.type = "button";
      element.className = "window-titlebar__item";
      element.setAttribute("role", "menuitem");
      element.setAttribute("aria-disabled", "true");
      const label = document.createElement("span");
      label.className = "window-titlebar__item-label";
      label.textContent = labelOf(def);
      element.appendChild(label);
      if (def.shortcut) {
        const shortcut = document.createElement("span");
        shortcut.className = "window-titlebar__shortcut";
        shortcut.textContent = def.shortcut;
        element.appendChild(shortcut);
      }
      const row: CommandRow = { element, labelElement: label, def };
      element.addEventListener("click", () => activateRow(row));
      rows.push(row);
      popover.appendChild(element);
    }
    button.insertAdjacentElement("afterend", popover);
    return { id, button, popover, rows };
  }

  function focusRow(handle: MenuHandle, index: number): void {
    const row = handle.rows[index];
    if (row) {
      row.element.focus();
    }
  }

  function openMenu(id: MenuId, focusFirst: boolean): void {
    closeMenu();
    const handle = handleFor(id);
    refreshEnabled(handle);
    // Align the popover under its button; absolute positioning keeps the
    // bar's layout unchanged.
    handle.popover.style.left = `${handle.button.offsetLeft}px`;
    handle.popover.hidden = false;
    handle.button.setAttribute("aria-expanded", "true");
    openId = id;
    if (focusFirst) {
      focusRow(handle, 0);
    }
  }

  function closeMenu(): void {
    if (openId === null) {
      return;
    }
    const handle = handleFor(openId);
    handle.popover.hidden = true;
    handle.button.setAttribute("aria-expanded", "false");
    openId = null;
  }

  function moveFocus(handle: MenuHandle, delta: number): void {
    const count = handle.rows.length;
    if (count === 0) {
      return;
    }
    const current = handle.rows.findIndex((row) => row.element === document.activeElement);
    const next =
      current === -1 ? (delta > 0 ? 0 : count - 1) : (current + delta + count) % count;
    focusRow(handle, next);
  }

  function stepMenu(delta: number): void {
    if (openId === null) {
      return;
    }
    const current = MENU_ORDER.indexOf(openId);
    const next = MENU_ORDER[(current + delta + MENU_ORDER.length) % MENU_ORDER.length];
    if (next) {
      openMenu(next, true);
    }
  }

  for (const id of MENU_ORDER) {
    handles.push(buildPopover(id));
  }

  for (const handle of handles) {
    handle.button.addEventListener("click", () => {
      if (openId === handle.id) {
        closeMenu();
      } else {
        openMenu(handle.id, false);
      }
    });
  }

  document.addEventListener("pointerdown", (event) => {
    if (openId === null) {
      return;
    }
    const handle = handleFor(openId);
    const target = event.target;
    if (!(target instanceof Node)) {
      return;
    }
    if (handle.popover.contains(target) || handle.button.contains(target)) {
      return;
    }
    closeMenu();
  });

  document.addEventListener("keydown", (event) => {
    if (openId === null) {
      return;
    }
    const handle = handleFor(openId);
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        closeMenu();
        handle.button.focus();
        break;
      case "ArrowDown":
        event.preventDefault();
        moveFocus(handle, 1);
        break;
      case "ArrowUp":
        event.preventDefault();
        moveFocus(handle, -1);
        break;
      case "ArrowRight":
        event.preventDefault();
        stepMenu(1);
        break;
      case "ArrowLeft":
        event.preventDefault();
        stepMenu(-1);
        break;
      case "Enter": {
        const row = handle.rows.find((candidate) => candidate.element === document.activeElement);
        if (row) {
          event.preventDefault();
          activateRow(row);
        }
        break;
      }
    }
  });

  return commands;
}
