// The application menus' composition root. The implementation is split
// across the three registry modules - command-registry.ts (command
// descriptors keyed by id), menu-registry.ts (placements per menu id),
// and menu-renderer.ts (reads the registries, builds and drives the
// popovers, knows nothing about what is registered). This file wires the
// workshop's own commands and placements into the shared registries and
// starts the renderer.
//
// The menus exist in both the desktop shell and a plain browser, since
// the title bar is always visible; only the native window commands
// (Window menu's Minimize/Maximize, File menu's Close Window) are inert
// in a browser, where no IPC bridge carries them. The Window menu reuses
// the window command functions from window-chrome.ts. The Model menu is
// dynamic: a provider rebuilds its rows from the model service's catalog
// on every open, and again on every workbench snapshot that arrives while
// it stays open.
//
// Edit commands go through document.execCommand: WebView2 hosts the page
// as application content with clipboard access, and execCommand preserves
// the editable target's native undo stack and selection semantics, which
// the async Clipboard API cannot. jsdom leaves execCommand undefined; the
// guard keeps the command a no-op there.

import type { Event } from "../../base/event";
import { DisposableStore, toDisposable, type IDisposable } from "../../base/lifecycle";
import type { CatalogModel } from "../../services/protocol";
import type { UpdateService } from "../../services/update-service";
import { showAboutDialog } from "../chrome/about-dialog";
import { closeWindow, minimizeWindow, toggleWindowMaximize } from "../chrome/window-chrome";
import { resetZoom, zoomIn, zoomOut } from "../chrome/zoom";
import { Commands, type CommandDescriptor } from "./command-registry";
import { Menus, type ResolvedMenuItem } from "./menu-registry";
import { MenuRenderer } from "./menu-renderer";

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
 * focuses the ws-agent-session panel. Agent windows are modal - one session
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
 * through: the menu lists "No profile" and then every profile, the
 * active selection checked, and selecting another asks the gateway to
 * switch. The state is read at open time and re-read on every
 * onDidChange while the menu stays open, so a switch's progress shows
 * without reopening.
 */
export interface ProfileMenuService {
  /** Every profile the gateway can load, by name. */
  readonly profiles: readonly string[];
  /** The active profile's name, or "" when no profile is active. */
  readonly active: string;
  /**
   * The profile a switch is loading, or "" both when no switch is running
   * and when the in-flight switch selects no profile.
   */
  readonly switching: string;
  /** Whether a switch is in flight, whatever its target. */
  readonly switchInFlight: boolean;
  /**
   * Fires when the state behind this view changes; the open Model
   * popover rebuilds its rows on each firing. Absent on a static view.
   */
  readonly onDidChange?: Event<unknown>;
  /** Asks the gateway to select the named profile, or no profile for null. */
  switchTo(name: string | null): void;
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

/**
 * The Model menu's rows mirror the catalog at open time: one checkable
 * radio row per model, the description as the tooltip, and a single
 * disabled row when the catalog is empty or no service was provided.
 * Below the models, a Profiles section lists "No profile" and then the
 * gateway's loadable profiles the same way; selecting one switches the
 * whole catalog. While a switch is loading, every row - model and
 * profile alike - disables (the rows describe a catalog about to be
 * replaced) and the switch target shows a pending mark where its check
 * would land.
 */
function modelMenuItems(
  service: ModelMenuService | undefined,
  profileService: ProfileMenuService | undefined,
): readonly ResolvedMenuItem[] {
  const isIdle = (): boolean => !profileService?.switchInFlight;
  const items: ResolvedMenuItem[] = [];
  const radio = (
    key: string,
    label: string,
    isSelected: boolean,
    isPending: boolean,
    tooltip: string | undefined,
    run: () => void,
  ): ResolvedMenuItem => ({
    kind: "command",
    key,
    label,
    run,
    enabled: isIdle,
    checked: isSelected,
    pending: isPending,
    tooltip,
  });
  const models = service ? service.models : [];
  if (!service || models.length === 0) {
    items.push({
      kind: "command",
      key: "empty",
      label: "No models available",
      run: () => {},
      enabled: () => false,
    });
  } else {
    const selected = service.current;
    for (const model of models) {
      items.push(
        radio(`model:${model.id}`, model.id, model.id === selected, false, model.description, () =>
          service.setCurrent(model.id),
        ),
      );
    }
  }
  // The Profiles section only appears when the gateway actually offers a
  // choice: with at least one profile defined, "No profile" is always the
  // alternative, so a single-profile gateway shows the section too. A
  // profile-less gateway keeps the menu as it was.
  const profiles = profileService ? profileService.profiles : [];
  if (profileService && profiles.length >= 1) {
    items.push({ kind: "separator" });
    items.push({
      kind: "command",
      key: "profiles-header",
      label: "Profiles",
      run: () => {},
      enabled: () => false,
    });
    items.push(
      radio(
        "profile:",
        "No profile",
        profileService.active === "",
        !isIdle() && !profileService.switching,
        "Serve remote models only; load no local models",
        () => profileService.switchTo(null),
      ),
    );
    for (const profile of profiles) {
      items.push(
        radio(
          `profile:${profile}`,
          profile,
          profile === profileService.active,
          !isIdle() && profile === profileService.switching,
          undefined,
          () => profileService.switchTo(profile),
        ),
      );
    }
  }
  return items;
}

/**
 * Builds the shared command set, registers the built-in menus and their
 * items into the command and menu registries, and starts the renderer on
 * the title bar - in the desktop shell and in a plain browser alike.
 * Native window commands (Minimize, Maximize/Restore, Close Window)
 * no-op without the IPC bridge; every other command works in both modes.
 * Throws if the title-bar markup is missing. The returned dispose()
 * releases the renderer's document-level listeners and removes the
 * popovers (whose removal also drops every row's element-owned click
 * listener). Registrations upsert, so a repeated setup (a test scenario)
 * replaces the previous commands and items instead of duplicating them.
 */
export function setupWindowMenus(options: {
  readonly agents: AgentMenuCommands;
  readonly workshop: WorkshopMenuCommands;
  /** The shared model state the dynamic Model menu reads and writes. */
  readonly modelMenu?: ModelMenuService;
  /** The gateway-profile state the Model menu's Profiles section reads. */
  readonly profileMenu?: ProfileMenuService;
  /** Desktop update state shown by the About dialog. */
  readonly updates?: UpdateService;
}): WindowMenuCommands & IDisposable {
  const nav = document.querySelector<HTMLElement>(".ws-window-titlebar__menus");
  if (!nav) {
    throw new Error("DOM Error: .ws-window-titlebar__menus not found in the page.");
  }

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
    showAbout: () => showAboutDialog(options.updates),
  };

  // The built-in commands, keyed by the ids the menu placements and the
  // shortcut dispatcher reference.
  const descriptors: Record<string, CommandDescriptor> = {
    "file.newAgent": { label: "New Agent", run: commands.newAgent },
    "file.closeWindow": { label: "Close Window", shortcut: "Alt+F4", run: commands.closeWindow },
    "edit.undo": { label: "Undo", shortcut: "Ctrl+Z", run: commands.undo, enabled: hasEditTarget },
    "edit.redo": { label: "Redo", shortcut: "Ctrl+Y", run: commands.redo, enabled: hasEditTarget },
    "edit.cut": { label: "Cut", shortcut: "Ctrl+X", run: commands.cut, enabled: hasEditTarget },
    "edit.copy": { label: "Copy", shortcut: "Ctrl+C", run: commands.copy, enabled: hasEditTarget },
    "edit.paste": { label: "Paste", shortcut: "Ctrl+V", run: commands.paste, enabled: hasEditTarget },
    "edit.selectAll": {
      label: "Select All",
      shortcut: "Ctrl+A",
      run: commands.selectAll,
      enabled: hasEditTarget,
    },
    "window.toggleWorkshopPanel": {
      label: "Workshop Panel",
      shortcut: "Ctrl+B",
      run: commands.toggleWorkshopPanel,
    },
    "window.openGatewayConfig": { label: "Gateway Config", run: commands.openGatewayConfig },
    "window.openAgentSession": { label: "New Agent", run: commands.openAgentSession },
    "window.zoomIn": { label: "Zoom In", shortcut: "Ctrl+=", run: commands.zoomIn },
    "window.zoomOut": { label: "Zoom Out", shortcut: "Ctrl+-", run: commands.zoomOut },
    "window.resetZoom": { label: "Reset Zoom", shortcut: "Ctrl+0", run: commands.resetZoom },
    "window.minimize": { label: "Minimize", run: commands.minimizeWindow },
    "window.toggleMaximize": { label: "Maximize/Restore", run: commands.toggleWindowMaximize },
    "help.about": { label: "About PromptForge", run: commands.showAbout },
  };
  for (const [id, descriptor] of Object.entries(descriptors)) {
    Commands.register(id, descriptor);
  }

  Menus.registerMenu("file", "File", 1);
  Menus.registerMenu("edit", "Edit", 2);
  Menus.registerMenu("model", "Model", 3);
  Menus.registerMenu("window", "Window", 4);
  Menus.registerMenu("help", "Help", 5);

  const item = (menu: string, command: string): void => {
    Menus.setMenuItem(menu, command, { kind: "command", command });
  };
  const separator = (menu: string, id: string): void => {
    Menus.setMenuItem(menu, `${menu}.${id}`, { kind: "separator" });
  };
  item("file", "file.newAgent");
  separator("file", "sep1");
  item("file", "file.closeWindow");
  item("edit", "edit.undo");
  item("edit", "edit.redo");
  separator("edit", "sep1");
  item("edit", "edit.cut");
  item("edit", "edit.copy");
  item("edit", "edit.paste");
  separator("edit", "sep2");
  item("edit", "edit.selectAll");
  item("window", "window.toggleWorkshopPanel");
  item("window", "window.openGatewayConfig");
  item("window", "window.openAgentSession");
  separator("window", "sep1");
  item("window", "window.zoomIn");
  item("window", "window.zoomOut");
  item("window", "window.resetZoom");
  separator("window", "sep2");
  item("window", "window.minimize");
  item("window", "window.toggleMaximize");
  item("help", "help.about");

  // The Model menu's rows are dynamic: the provider re-reads the catalog
  // and profile surfaces at every open, and onDidChange rebuilds the rows
  // while the popover stays open.
  Menus.setProvider("model", {
    items: () => modelMenuItems(options.modelMenu, options.profileMenu),
    onDidChange: options.profileMenu?.onDidChange,
  });

  store.add(new MenuRenderer(nav));

  return { ...commands, dispose: (): void => store.dispose() };
}
