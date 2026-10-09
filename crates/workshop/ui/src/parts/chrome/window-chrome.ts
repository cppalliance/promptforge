// Custom window title bar. The bar is always shown, in the desktop app
// and in a plain browser, because it holds the application menus; only
// the native window controls (drag region, minimize/maximize/close) are
// desktop-only, since they need the Tauri window API. Every control calls
// the current window through @tauri-apps/api, which esbuild bundles the
// same way as the rest of the UI.
//
// The bar also carries two toolbars of icon buttons, built here from a
// fixed list of commands: Toggle Primary Side Bar after the menus on the
// left, and Toggle Agents and the settings gear before the window controls
// on the right. A press runs the command through the command registry, and
// the tooltip names its chord. The bar follows the window's focus too: while
// the window is blurred it wears the inactive modifier, which dims its text.

import "./window-chrome.css";

import { getCurrentWindow, type Window as TauriWindow } from "@tauri-apps/api/window";

import { ICON_GEAR, ICON_LAYOUT_SIDEBAR_LEFT, ICON_LAYOUT_SIDEBAR_RIGHT } from "@workshop/look/icons";
import { Commands, logCommandFailure, type CommandRegistry } from "@workshop/platform/command-registry";
import { DisposableStore, toDisposable, type IDisposable } from "@workshop/platform/lifecycle";
import { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
import { detectPlatform } from "@workshop/platform/keybinding-parser";
import { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
import { getService } from "@workshop/platform/service-registry";

declare global {
  interface Window {
    // Injected by the Tauri runtime in the desktop app; absent in a plain
    // browser, where the native window controls stay hidden.
    __TAURI_INTERNALS__?: unknown;
  }
}

/** The native window, or null in a plain browser where no window exists. */
function currentWindow(): TauriWindow | null {
  return window.__TAURI_INTERNALS__ === undefined ? null : getCurrentWindow();
}

/**
 * Runs one native window command. In a plain browser the command has no
 * window to act on; dropping it beats throwing from a click. A rejected
 * call in the desktop app is a packaging defect (a missing capability), so
 * it is logged rather than swallowed.
 */
function runWindowCommand(run: (window: TauriWindow) => Promise<void>): void {
  const win = currentWindow();
  if (win === null) {
    return;
  }
  void run(win).catch((error: unknown) => {
    console.error("a native window command failed:", error);
  });
}

/** Minimizes the window. Shared by the visible control and the Window menu. */
export function minimizeWindow(): void {
  runWindowCommand((win) => win.minimize());
}

/** Toggles between maximized and restored. Shared by the visible control, the drag region double-click, and the Window menu. */
export function toggleWindowMaximize(): void {
  runWindowCommand((win) => win.toggleMaximize());
}

/** Closes the window. Shared by the visible control and the File menu. */
export function closeWindow(): void {
  runWindowCommand((win) => win.close());
}

/** Toggles native fullscreen. The Appearance menu's Full Screen row and F11 dispatch here. */
export function toggleFullScreen(): void {
  runWindowCommand(async (win) => {
    await win.setFullscreen(!(await win.isFullscreen()));
  });
}

/** Registry overrides; tests inject their own. */
export interface WindowChromeDependencies {
  readonly commands?: CommandRegistry;
  readonly keybindings?: KeybindingsRegistry;
}

/** One toolbar icon button: the command it runs, its accessible name, and its glyph. */
interface ToolButtonSpec {
  readonly commandId: string;
  readonly label: string;
  readonly icon: string;
}

/** The left toolbar, after the menubar. */
const LEFT_TOOLS: readonly ToolButtonSpec[] = [
  { commandId: "workbench.action.toggleSidebarVisibility", label: "Toggle Primary Side Bar", icon: ICON_LAYOUT_SIDEBAR_LEFT },
];

/** The right toolbar, before the window controls: Toggle Agents (the right zone) and the settings gear. */
const RIGHT_TOOLS: readonly ToolButtonSpec[] = [
  { commandId: "workbench.action.toggleAuxiliaryBar", label: "Toggle Agents", icon: ICON_LAYOUT_SIDEBAR_RIGHT },
  { commandId: "workbench.action.openSettings", label: "Settings", icon: ICON_GEAR },
];

/** The active-window state's modifier class on the bar. */
const INACTIVE_CLASS = "ws-window-titlebar--inactive";

/**
 * Builds one toolbar of icon buttons. A press runs the command; a failure
 * posts to the status bar, the way every other surface reports one. The
 * tooltip is the label plus the command's chord, re-read on hover and focus
 * because the contributions that bind chords may register after boot.
 */
function buildToolbar(
  tools: readonly ToolButtonSpec[],
  commands: CommandRegistry,
  keybindings: KeybindingsRegistry,
): HTMLElement {
  const toolbar = document.createElement("div");
  toolbar.className = "ws-window-titlebar__toolbar";
  for (const tool of tools) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "ws-window-titlebar__tool";
    button.dataset["commandId"] = tool.commandId;
    button.setAttribute("aria-label", tool.label);
    button.innerHTML = tool.icon;
    const refreshTooltip = (): void => {
      const chord = keybindings.lookupKeybinding(tool.commandId)?.getLabel();
      button.title = chord === undefined ? tool.label : `${tool.label} (${chord})`;
    };
    refreshTooltip();
    button.addEventListener("pointerenter", refreshTooltip);
    button.addEventListener("focus", refreshTooltip);
    button.addEventListener("click", () => {
      // The registry reports a failure (the toast); keep the detail.
      void commands.execute(tool.commandId).catch((error: unknown) => {
        logCommandFailure(tool.commandId, error);
      });
    });
    toolbar.appendChild(button);
  }
  return toolbar;
}

/**
 * Reveals the custom title bar in every mode: the bar holds the
 * application menus, so it must show in a plain browser too. The drag
 * region, the window controls, and the maximized-state sync are wired
 * only inside the desktop app; in a browser the control cluster is
 * hidden instead, since the commands would have no window to reach.
 * The toolbars and the inactive-window mark are wired in every mode.
 * The menu buttons are wired to their popovers by `setupWindowMenus` in
 * parts/menu/index.ts. Returns the disposable owning every listener wired here.
 */
export function setupWindowChrome(deps: WindowChromeDependencies = {}): IDisposable {
  const store = new DisposableStore();
  const bar = document.querySelector<HTMLElement>(".ws-window-titlebar");
  if (!bar) {
    throw new Error("DOM Error: .ws-window-titlebar not found in the page.");
  }
  const controls = bar.querySelector<HTMLElement>(".ws-window-titlebar__controls");
  if (!controls) {
    throw new Error("DOM Error: the title bar is missing its window-control cluster.");
  }
  const leftRegion = bar.querySelector<HTMLElement>(".ws-window-titlebar__left");
  const rightRegion = bar.querySelector<HTMLElement>(".ws-window-titlebar__right");
  if (!leftRegion || !rightRegion) {
    throw new Error("DOM Error: the title bar is missing its left or right region.");
  }

  bar.hidden = false;

  const commands = deps.commands ?? Commands;
  const keybindings = deps.keybindings ?? KeybindingsRegistry;
  const leftToolbar = buildToolbar(LEFT_TOOLS, commands, keybindings);
  const rightToolbar = buildToolbar(RIGHT_TOOLS, commands, keybindings);
  leftRegion.appendChild(leftToolbar);
  rightRegion.insertBefore(rightToolbar, controls);
  store.add(
    toDisposable(() => {
      leftToolbar.remove();
      rightToolbar.remove();
    }),
  );

  // Inactive window: the bar dims while the window is blurred. The bar
  // starts active; the first blur or focus event is the first signal.
  const onBlur = (): void => {
    bar.classList.add(INACTIVE_CLASS);
  };
  const onFocus = (): void => {
    bar.classList.remove(INACTIVE_CLASS);
  };
  window.addEventListener("blur", onBlur);
  window.addEventListener("focus", onFocus);
  store.add(
    toDisposable(() => {
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
    }),
  );

  const win = currentWindow();
  // The platform context keys the menu rows read: isWeb gates the
  // desktop-only rows (Full Screen, Close Window), isFullscreen drives
  // the Full Screen checkbox. Both bind at boot, before any menu opens.
  const contextKeys = getService(CONTEXT_KEY_SERVICE);
  const fullscreenKey = contextKeys.createKey("isFullscreen", false);
  contextKeys.createKey("isWeb", win === null);
  if (win === null) {
    // No native window exists for the buttons to act on; showing them
    // would present dead controls.
    controls.hidden = true;
    return store;
  }
  // macOS overlay chrome: the desktop app runs the window with titleBarStyle
  // Overlay and a hidden title, so the native traffic lights float over
  // the bar's left edge and cover close/minimize/zoom. The custom
  // Windows-style cluster would double them, so it hides, and the bar
  // takes the class whose CSS insets the left region clear of the lights.
  // The drag region and the state syncs stay: the empty center still
  // drags, and the green light's native fullscreen is what isFullscreen
  // tracks.
  if (detectPlatform() === "mac") {
    controls.hidden = true;
    bar.classList.add("ws-window-titlebar--macos");
  }
  const drag = bar.querySelector<HTMLElement>(".ws-window-titlebar__drag");
  const minimize = bar.querySelector<HTMLButtonElement>('[data-command="minimize"]');
  const maximize = bar.querySelector<HTMLButtonElement>('[data-command="toggle-maximize"]');
  const close = bar.querySelector<HTMLButtonElement>('[data-command="close"]');
  if (!drag || !minimize || !maximize || !close) {
    throw new Error("DOM Error: the title bar is missing a drag region or a window control.");
  }
  // The glyphs are <svg>, which has no `hidden` IDL attribute, so visibility
  // is toggled through the content attribute (with a matching [hidden] rule
  // in window-chrome.css, since the UA rule covers only HTML elements).
  const maximizeGlyph = maximize.querySelector<SVGSVGElement>(".ws-window-titlebar__glyph--maximize");
  const restoreGlyph = maximize.querySelector<SVGSVGElement>(".ws-window-titlebar__glyph--restore");
  if (!maximizeGlyph || !restoreGlyph) {
    throw new Error("DOM Error: the maximize control is missing its glyphs.");
  }

  minimize.addEventListener("click", minimizeWindow);
  store.add(toDisposable(() => minimize.removeEventListener("click", minimizeWindow)));
  maximize.addEventListener("click", toggleWindowMaximize);
  store.add(toDisposable(() => maximize.removeEventListener("click", toggleWindowMaximize)));
  close.addEventListener("click", closeWindow);
  store.add(toDisposable(() => close.removeEventListener("click", closeWindow)));

  // Only the empty center drags; the buttons handle their own presses.
  // `startDragging` hands the mouse to the OS move loop, so the webview
  // never sees the release and a `dblclick` can never be synthesized.
  // The double-click is therefore read from the press itself: `mousedown`
  // sets the click count in `detail` (a `pointerdown` always reports
  // 0), and the second press toggles maximize instead of starting a drag.
  // This mirrors the drag-region script Tauri injects for
  // `data-tauri-drag-region`.
  const onDragMouseDown = (event: MouseEvent): void => {
    if (event.button !== 0 || event.target !== drag) {
      return;
    }
    if (event.detail === 2) {
      toggleWindowMaximize();
    } else {
      runWindowCommand((win) => win.startDragging());
    }
  };
  drag.addEventListener("mousedown", onDragMouseDown);
  store.add(toDisposable(() => drag.removeEventListener("mousedown", onDragMouseDown)));

  // The maximize/restore glyph follows the window's maximized state, read
  // back after every resize (every maximize path - button, double-click,
  // Windows Snap, restore - surfaces as a resize). The DOM is touched only
  // on transitions: a drag-resize streams resize events while the flag
  // almost never changes.
  let lastMaximized: boolean | null = null;
  const syncMaximized = async (): Promise<void> => {
    const maximized = await win.isMaximized();
    if (maximized === lastMaximized) {
      return;
    }
    lastMaximized = maximized;
    maximize.setAttribute("aria-label", maximized ? "Restore" : "Maximize");
    maximizeGlyph.toggleAttribute("hidden", maximized);
    restoreGlyph.toggleAttribute("hidden", !maximized);
  };
  void syncMaximized();
  // The fullscreen checkbox follows the window the same way: every
  // transition (F11, the macOS green light, the menu row) is a resize.
  const syncFullscreen = async (): Promise<void> => {
    fullscreenKey.set(await win.isFullscreen());
  };
  void syncFullscreen();
  const unlisten = win.onResized(() => {
    void syncMaximized();
    void syncFullscreen();
  });
  store.add(
    toDisposable(() => {
      void unlisten.then((off) => off());
    }),
  );
  return store;
}
