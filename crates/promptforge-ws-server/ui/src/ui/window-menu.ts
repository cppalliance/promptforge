// Application menus for the custom title bar: accessible HTML popovers
// behind the File, Edit, Model, Window, and Help buttons, not native
// menus. The menus exist in both the desktop shell and a plain browser,
// since the title bar is always visible; only the native window commands
// (Window menu's Minimize/Maximize, File menu's Close Window) are inert
// in a browser, where no IPC bridge carries them. Every command
// dispatches through the single WindowMenuCommands set built here, so
// the popovers, future keyboard shortcuts, and any future menu surface
// all call the same actions. The Window menu reuses the window command
// functions from window-chrome.ts. The Model menu is dynamic: it
// rebuilds its rows from the model service's catalog on every open.
//
// Edit commands go through document.execCommand: WebView2 hosts the page
// as application content with clipboard access, and execCommand preserves
// the editable target's native undo stack and selection semantics, which
// the async Clipboard API cannot. jsdom leaves execCommand undefined; the
// guard keeps the command a no-op there.

import { showAboutDialog } from "./about-dialog";
import type { ModelService } from "../services/model-service";
import { closeWindow, minimizeWindow, toggleWindowMaximize } from "./window-chrome";

/** The actions every menu surface and keyboard shortcut dispatches through. */
export interface WindowMenuCommands {
  readonly newAgent: () => void;
  readonly closeWindow: () => void;
  readonly undo: () => void;
  readonly redo: () => void;
  readonly cut: () => void;
  readonly copy: () => void;
  readonly paste: () => void;
  readonly selectAll: () => void;
  readonly toggleWorkshopPanel: () => void;
  readonly minimizeWindow: () => void;
  readonly toggleWindowMaximize: () => void;
  readonly showAbout: () => void;
}

/**
 * The workshop surface the Window menu dispatches through: the Workshop
 * Panel item toggles the tree, sharing the Ctrl+B command from
 * workshop/shortcuts.
 */
export interface WorkshopMenuCommands {
  readonly toggleWorkshopPanel: () => void;
}

/**
 * The agent surface the File menu dispatches through: New Agent opens a
 * fresh Agent tab, the only way to start a new conversation. The Agent
 * panel controller satisfies this structurally.
 */
export interface AgentMenuCommands {
  readonly newAgent: () => void;
}

const MENU_ORDER = ["file", "edit", "model", "window", "help"] as const;
type MenuId = (typeof MENU_ORDER)[number];

const MENU_LABELS: Record<MenuId, string> = {
  file: "File",
  edit: "Edit",
  model: "Model",
  window: "Window",
  help: "Help",
};

interface CommandItem {
  readonly kind: "command";
  readonly label: string;
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
  // Mutable: the Model menu rebuilds its rows from the catalog on every
  // open, so the array cannot be frozen at build time.
  readonly rows: CommandRow[];
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

function buildMenuItems(
  commands: WindowMenuCommands,
  hasEditTarget: () => boolean,
): Record<MenuId, readonly MenuItem[]> {
  const windowItems: MenuItem[] = [
    { kind: "command", label: "Workshop Panel", shortcut: "Ctrl+B", run: commands.toggleWorkshopPanel },
    { kind: "separator" },
    { kind: "command", label: "Minimize", run: commands.minimizeWindow },
    { kind: "command", label: "Maximize/Restore", run: commands.toggleWindowMaximize },
  ];
  return {
    file: [
      { kind: "command", label: "New Agent", run: commands.newAgent },
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
    // The Model menu's rows are dynamic; rebuildModelRows fills them in
    // from the catalog surface on every open.
    model: [],
  };
}

/**
 * Builds the shared command set and wires the title-bar menu buttons to
 * their popovers, in the desktop shell and in a plain browser alike.
 * Native window commands (Minimize, Maximize/Restore, Close Window)
 * no-op without the IPC bridge; every other command works in both modes.
 * Throws if the title-bar markup is missing.
 */
export function setupWindowMenus(options: {
  readonly agents: AgentMenuCommands;
  readonly workshop: WorkshopMenuCommands;
  /** The shared model state the dynamic Model menu reads and writes. */
  readonly modelMenu?: ModelService;
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
    // Wrapped, not aliased: the agent surface may be a class instance
    // whose methods need their receiver.
    newAgent: () => options.agents.newAgent(),
    closeWindow,
    undo: () => runEditCommand("undo"),
    redo: () => runEditCommand("redo"),
    cut: () => runEditCommand("cut"),
    copy: () => runEditCommand("copy"),
    paste: () => runEditCommand("paste"),
    selectAll: () => runEditCommand("selectAll"),
    toggleWorkshopPanel: () => options.workshop.toggleWorkshopPanel(),
    minimizeWindow,
    toggleWindowMaximize,
    showAbout: showAboutDialog,
  };

  const items = buildMenuItems(commands, hasEditTarget);
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
      row.labelElement.textContent = row.def.label;
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

  // One command row: a menuitem button with its label, an optional
  // shortcut hint, and click dispatch through activateRow. extras runs
  // before the label is appended, so leading affordances (the Model
  // menu's check column) land before the label in layout order.
  function buildCommandRow(
    def: CommandItem,
    extras?: (element: HTMLButtonElement) => void,
  ): CommandRow {
    const element = document.createElement("button");
    element.type = "button";
    element.className = "window-titlebar__item";
    element.setAttribute("role", "menuitem");
    element.setAttribute("aria-disabled", "true");
    const label = document.createElement("span");
    label.className = "window-titlebar__item-label";
    label.textContent = def.label;
    extras?.(element);
    element.appendChild(label);
    if (def.shortcut) {
      const shortcut = document.createElement("span");
      shortcut.className = "window-titlebar__shortcut";
      shortcut.textContent = def.shortcut;
      element.appendChild(shortcut);
    }
    const row: CommandRow = { element, labelElement: label, def };
    element.addEventListener("click", () => activateRow(row));
    return row;
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
      const row = buildCommandRow(def);
      rows.push(row);
      popover.appendChild(row.element);
    }
    button.insertAdjacentElement("afterend", popover);
    return { id, button, popover, rows };
  }

  // The Model menu's rows mirror the catalog at open time: one checkable
  // radio row per model, the description as the tooltip, and a single
  // disabled row when the catalog is empty or no service was provided.
  function rebuildModelRows(handle: MenuHandle): void {
    const service = options.modelMenu;
    handle.popover.textContent = "";
    handle.rows.length = 0;
    const models = service ? service.models : [];
    const appendRow = (def: CommandItem, extras: (element: HTMLButtonElement) => void): void => {
      const row = buildCommandRow(def, extras);
      handle.rows.push(row);
      handle.popover.appendChild(row.element);
    };
    if (!service || models.length === 0) {
      appendRow(
        { kind: "command", label: "No models available", run: () => {}, enabled: () => false },
        () => {},
      );
      return;
    }
    const selected = service.current;
    for (const model of models) {
      const isSelected = model.id === selected;
      appendRow(
        { kind: "command", label: model.id, run: () => service.setCurrent(model.id) },
        (element) => {
          element.classList.add("window-titlebar__item--checkable");
          element.setAttribute("role", "menuitemradio");
          element.setAttribute("aria-checked", isSelected ? "true" : "false");
          if (model.description) {
            element.title = model.description;
          }
          const check = document.createElement("span");
          check.className = "window-titlebar__item-check";
          check.setAttribute("aria-hidden", "true");
          check.textContent = isSelected ? "✓" : "";
          element.appendChild(check);
        },
      );
    }
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
    if (id === "model") {
      rebuildModelRows(handle);
    }
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
