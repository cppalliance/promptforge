// Custom window title bar. The bar is always shown, in the desktop app
// and in a plain browser, because it carries the application menus; only
// the native window controls (drag region, minimize/maximize/close) are
// desktop-only, since they need the Tauri window API. Every control calls
// the current window through @tauri-apps/api, which esbuild bundles the
// same way as the rest of the UI.

import "./window-chrome.css";

import { getCurrentWindow, type Window as TauriWindow } from "@tauri-apps/api/window";

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";

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

/**
 * Reveals the custom title bar in every mode: the bar carries the
 * application menus, so it must show in a plain browser too. The drag
 * region, the window controls, and the maximized-state sync are wired
 * only inside the desktop app; in a browser the control cluster is
 * hidden instead, since the commands would have no window to reach.
 * The menu buttons are wired to their popovers by `setupWindowMenus` in
 * window-menu.ts. Returns the disposable owning every listener wired here.
 */
export function setupWindowChrome(): IDisposable {
  const store = new DisposableStore();
  const bar = document.querySelector<HTMLElement>(".window-titlebar");
  if (!bar) {
    throw new Error("DOM Error: .window-titlebar not found in the page.");
  }
  const controls = bar.querySelector<HTMLElement>(".window-titlebar__controls");
  if (!controls) {
    throw new Error("DOM Error: the title bar is missing its window-control cluster.");
  }

  bar.hidden = false;

  const win = currentWindow();
  if (win === null) {
    // No native window exists for the buttons to act on; showing them
    // would present dead controls.
    controls.hidden = true;
    return store;
  }
  const drag = bar.querySelector<HTMLElement>(".window-titlebar__drag");
  const minimize = bar.querySelector<HTMLButtonElement>('[data-command="minimize"]');
  const maximize = bar.querySelector<HTMLButtonElement>('[data-command="toggle-maximize"]');
  const close = bar.querySelector<HTMLButtonElement>('[data-command="close"]');
  if (!drag || !minimize || !maximize || !close) {
    throw new Error("DOM Error: the title bar is missing a drag region or a window control.");
  }
  // The glyphs are <svg>, which has no `hidden` IDL attribute, so visibility
  // is toggled through the content attribute (with a matching [hidden] rule
  // in window-chrome.css, since the UA rule covers only HTML elements).
  const maximizeGlyph = maximize.querySelector<SVGSVGElement>(".window-titlebar__glyph--maximize");
  const restoreGlyph = maximize.querySelector<SVGSVGElement>(".window-titlebar__glyph--restore");
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
  const onDragPointerDown = (event: PointerEvent): void => {
    if (event.button === 0 && event.target === drag) {
      runWindowCommand((win) => win.startDragging());
    }
  };
  drag.addEventListener("pointerdown", onDragPointerDown);
  store.add(toDisposable(() => drag.removeEventListener("pointerdown", onDragPointerDown)));
  drag.addEventListener("dblclick", toggleWindowMaximize);
  store.add(toDisposable(() => drag.removeEventListener("dblclick", toggleWindowMaximize)));

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
  const unlisten = win.onResized(() => {
    void syncMaximized();
  });
  store.add(
    toDisposable(() => {
      void unlisten.then((off) => off());
    }),
  );
  return store;
}
