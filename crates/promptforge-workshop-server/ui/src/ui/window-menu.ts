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
// rebuilds its rows from the model service's catalog on every open, and
// again on every workbench snapshot that arrives while it stays open.
//
// Edit commands go through document.execCommand: WebView2 hosts the page
// as application content with clipboard access, and execCommand preserves
// the editable target's native undo stack and selection semantics, which
// the async Clipboard API cannot. jsdom leaves execCommand undefined; the
// guard keeps the command a no-op there.

import "./window-menu.css";

import type { Event } from "../base/event";
import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";
import { showAboutDialog } from "./about-dialog";
import type { CatalogModel } from "../services/protocol";
import { closeWindow, minimizeWindow, toggleWindowMaximize } from "./window-chrome";
import { resetZoom, zoomIn, zoomOut } from "./zoom";

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
  readonly openGatewayConfig: () => void;
  readonly openAgentSession: () => void;
  readonly minimizeWindow: () => void;
  readonly toggleWindowMaximize: () => void;
  readonly zoomIn: () => void;
  readonly zoomOut: () => void;
  readonly resetZoom: () => void;
  readonly showAbout: () => void;
}

/**
 * The workshop surface the Window menu dispatches through: the Workshop
 * Panel item toggles the tree, sharing the Ctrl+B command from
 * workshop/shortcuts, Gateway Config opens (or focuses) the dockview
 * panel hosting the gateway's config SPA, and New Agent opens (or
 * focuses) the agent-session panel and starts a fresh session.
 */
export interface WorkshopMenuCommands {
  readonly toggleWorkshopPanel: () => void;
  readonly openGatewayConfig: () => void;
  readonly openAgentSession: () => void;
}

/**
 * The agent surface the File menu dispatches through: New Agent opens or
 * focuses the agent-session panel. Agent windows are modal - one session
 * per window - so the panel is a singleton and reopening focuses it.
 */
export interface AgentMenuCommands {
  readonly newAgent: () => void;
}

/**
 * The model-catalog surface the Model menu reads and dispatches through:
 * the menu lists every catalog model with the selected one checked, and
 * clicking another asks the server to select it. setCurrent returns
 * nothing here: the view surfaces a failed send itself (the composition
 * root routes it to the status bar), the same seam
 * ProfileMenuService.switchTo uses for a failed switch.
 */
export interface ModelMenuService {
  /** The model catalog, as pushed by the server. */
  readonly models: readonly CatalogModel[];
  /** The selected model's id, or "" when no model is selected. */
  readonly current: string;
  /** Asks the server to select the model; the view surfaces a failed send. */
  setCurrent(id: string): void;
}

/**
 * The gateway-profile surface the Model menu reads and dispatches
 * through: the menu lists every profile with the active one checked, and
 * selecting another asks the gateway to switch. The state is read at
 * open time and re-read on every onDidChange while the menu stays open,
 * so a switch's progress shows without reopening.
 */
export interface ProfileMenuService {
  /** Every profile the gateway can load, by name. */
  readonly profiles: readonly string[];
  /** The active profile's name, or "" when unknown. */
  readonly active: string;
  /** The profile a switch is loading, or "" when no switch is running. */
  readonly switching: string;
  /**
   * Fires when the state behind this view changes; the open Model
   * popover rebuilds its rows on each firing. Absent on a static view.
   */
  readonly onDidChange?: Event<unknown>;
  /** Asks the gateway to switch to the named profile. */
  switchTo(name: string): void;
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
    { kind: "command", label: "Gateway Config", run: commands.openGatewayConfig },
    { kind: "command", label: "New Agent", run: commands.openAgentSession },
    { kind: "separator" },
    { kind: "command", label: "Zoom In", shortcut: "Ctrl+=", run: commands.zoomIn },
    { kind: "command", label: "Zoom Out", shortcut: "Ctrl+-", run: commands.zoomOut },
    { kind: "command", label: "Reset Zoom", shortcut: "Ctrl+0", run: commands.resetZoom },
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
 * Throws if the title-bar markup is missing. The returned dispose()
 * releases the document-level listeners and removes the popovers (whose
 * removal also drops every row's element-owned click listener).
 */
export function setupWindowMenus(options: {
  readonly agents: AgentMenuCommands;
  readonly workshop: WorkshopMenuCommands;
  /** The shared model state the dynamic Model menu reads and writes. */
  readonly modelMenu?: ModelMenuService;
  /** The gateway-profile state the Model menu's Profiles section reads. */
  readonly profileMenu?: ProfileMenuService;
}): WindowMenuCommands & IDisposable {
  const navElement = document.querySelector<HTMLElement>(".window-titlebar__menus");
  if (!navElement) {
    throw new Error("DOM Error: .window-titlebar__menus not found in the page.");
  }
  // A separate const: narrowing does not propagate into the hoisted
  // function declarations below.
  const nav: HTMLElement = navElement;

  const store = new DisposableStore();

  // Edit commands act on the editable element focused before the menu
  // opened. Clicking a menu button moves focus to the button, so the
  // target is remembered continuously instead of read at open time.
  let editTarget: HTMLElement | null = null;
  const onFocusIn = (event: FocusEvent): void => {
    const target = event.target instanceof Element ? event.target : null;
    if (isEditable(target)) {
      editTarget = target;
    }
  };
  document.addEventListener("focusin", onFocusIn);
  store.add(toDisposable(() => document.removeEventListener("focusin", onFocusIn)));
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
    openGatewayConfig: () => options.workshop.openGatewayConfig(),
    openAgentSession: () => options.workshop.openAgentSession(),
    minimizeWindow,
    toggleWindowMaximize,
    zoomIn,
    zoomOut,
    resetZoom,
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
    store.add(toDisposable(() => popover.remove()));
    return { id, button, popover, rows };
  }

  // The Model menu's rows mirror the catalog at open time: one checkable
  // radio row per model, the description as the tooltip, and a single
  // disabled row when the catalog is empty or no service was provided.
  // Below the models, a Profiles section lists the gateway's loadable
  // profiles the same way; selecting one switches the whole catalog.
  // While a switch is loading, every row - model and profile alike -
  // disables (the rows describe a catalog about to be replaced) and the
  // switch target shows a pending mark where its check would land.
  function rebuildModelRows(handle: MenuHandle): void {
    const service = options.modelMenu;
    const profileService = options.profileMenu;
    const isIdle = (): boolean => !profileService?.switching;
    // Wiping the popover destroys the focused row and drops keyboard
    // focus to body, so a snapshot arriving mid-navigation would yank a
    // screen-reader user's position. Remember the focused row by its
    // stable identity (model id / profile name, never index) and restore
    // focus onto the equivalent new row after the rebuild.
    const focusedKey = handle.rows.find(
      (row) => row.element === document.activeElement,
    )?.element.dataset["menuRowKey"];
    handle.popover.textContent = "";
    handle.rows.length = 0;
    const models = service ? service.models : [];
    const appendRow = (
      key: string,
      def: CommandItem,
      extras: (element: HTMLButtonElement) => void,
    ): void => {
      const row = buildCommandRow(def, extras);
      row.element.dataset["menuRowKey"] = key;
      handle.rows.push(row);
      handle.popover.appendChild(row.element);
    };
    const appendRadioRow = (
      key: string,
      label: string,
      isSelected: boolean,
      isPending: boolean,
      tooltip: string | undefined,
      run: () => void,
    ): void => {
      appendRow(key, { kind: "command", label, run, enabled: isIdle }, (element) => {
        element.classList.add("window-titlebar__item--checkable");
        element.setAttribute("role", "menuitemradio");
        element.setAttribute("aria-checked", isSelected ? "true" : "false");
        if (tooltip) {
          element.title = tooltip;
        }
        const check = document.createElement("span");
        check.className = "window-titlebar__item-check";
        if (isPending) {
          check.classList.add("window-titlebar__item-check--pending");
          // The "…" mark is aria-hidden and aria-checked stays false
          // until the server confirms, so without this the switch target
          // is indistinguishable from the other disabled rows for
          // assistive tech. The next settle rebuilds rows without it.
          element.setAttribute("aria-busy", "true");
        }
        check.setAttribute("aria-hidden", "true");
        check.textContent = isPending ? "…" : isSelected ? "✓" : "";
        element.appendChild(check);
      });
    };
    if (!service || models.length === 0) {
      appendRow(
        "empty",
        { kind: "command", label: "No models available", run: () => {}, enabled: () => false },
        () => {},
      );
    } else {
      const selected = service.current;
      for (const model of models) {
        appendRadioRow(
          `model:${model.id}`,
          model.id,
          model.id === selected,
          false,
          model.description,
          () => service.setCurrent(model.id),
        );
      }
    }
    // The Profiles section only appears when the gateway actually offers a
    // choice; a single-profile (or profile-less) gateway keeps the menu as
    // it was.
    const profiles = profileService ? profileService.profiles : [];
    if (profileService && profiles.length >= 2) {
      const separator = document.createElement("div");
      separator.className = "window-titlebar__separator";
      separator.setAttribute("role", "separator");
      handle.popover.appendChild(separator);
      appendRow(
        "profiles-header",
        { kind: "command", label: "Profiles", run: () => {}, enabled: () => false },
        () => {},
      );
      for (const profile of profiles) {
        appendRadioRow(
          `profile:${profile}`,
          profile,
          profile === profileService.active,
          !isIdle() && profile === profileService.switching,
          undefined,
          () => profileService.switchTo(profile),
        );
      }
    }
    // Only when a row held focus before the wipe: a rebuild while focus
    // is elsewhere must not grab it. The snapshot may have dropped the
    // focused row entirely; the first row is the fallback.
    if (focusedKey !== undefined) {
      const restored =
        handle.rows.find((row) => row.element.dataset["menuRowKey"] === focusedKey) ??
        handle.rows[0];
      restored?.element.focus();
    }
  }

  function focusRow(handle: MenuHandle, index: number): void {
    const row = handle.rows[index];
    if (row) {
      row.element.focus();
    }
  }

  // Alive only while the Model popover is open: each workbench snapshot
  // rebuilds the rows in place, so the check and pending marks move
  // without reopening. closeMenu and teardown both dispose it.
  let modelWatch: IDisposable | null = null;
  store.add(
    toDisposable(() => {
      modelWatch?.dispose();
      modelWatch = null;
    }),
  );

  function openMenu(id: MenuId, focusFirst: boolean): void {
    closeMenu();
    const handle = handleFor(id);
    if (id === "model") {
      rebuildModelRows(handle);
      modelWatch =
        options.profileMenu?.onDidChange?.(() => {
          rebuildModelRows(handle);
          refreshEnabled(handle);
        }) ?? null;
    }
    refreshEnabled(handle);
    // Align the popover under its button; absolute positioning keeps the
    // bar's layout unchanged.
    handle.popover.style.left = `${handle.button.offsetLeft}px`;
    handle.popover.style.top = `${handle.button.offsetTop + handle.button.offsetHeight}px`;
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
    modelWatch?.dispose();
    modelWatch = null;
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
    const onButtonClick = (): void => {
      if (openId === handle.id) {
        closeMenu();
      } else {
        openMenu(handle.id, false);
      }
    };
    handle.button.addEventListener("click", onButtonClick);
    store.add(toDisposable(() => handle.button.removeEventListener("click", onButtonClick)));
    // Menubar rollover: while any menu is open, hovering another button
    // switches the open menu to it (openMenu closes the current one
    // first). With no menu open, hover alone opens nothing.
    const onButtonEnter = (): void => {
      if (openId !== null && openId !== handle.id) {
        openMenu(handle.id, false);
      }
    };
    handle.button.addEventListener("pointerenter", onButtonEnter);
    store.add(toDisposable(() => handle.button.removeEventListener("pointerenter", onButtonEnter)));
  }

  const onPointerDown = (event: PointerEvent): void => {
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
  };
  document.addEventListener("pointerdown", onPointerDown);
  store.add(toDisposable(() => document.removeEventListener("pointerdown", onPointerDown)));

  // Close the menu when the window loses focus (Alt+Tab, taskbar click,
  // notification popup) so a stale popover never covers a returned window.
  const onWindowBlur = (): void => closeMenu();
  window.addEventListener("blur", onWindowBlur);
  store.add(toDisposable(() => window.removeEventListener("blur", onWindowBlur)));

  const onKeydown = (event: KeyboardEvent): void => {
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
  };
  document.addEventListener("keydown", onKeydown);
  store.add(toDisposable(() => document.removeEventListener("keydown", onKeydown)));

  return { ...commands, dispose: (): void => store.dispose() };
}
