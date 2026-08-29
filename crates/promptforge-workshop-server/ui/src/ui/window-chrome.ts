// Custom window title bar. The bar is always shown, in the desktop shell
// and in a plain browser, because it carries the application menus; only
// the native window controls (drag region, minimize/maximize/close) are
// desktop-only, since they need the wry IPC bridge. Every control sends a
// narrow, typed command through that bridge - the shell parses and
// validates the payload before any native window operation runs.

import "./window-chrome.css";

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";

declare global {
  interface Window {
    // Set by the wry initialization script in the desktop shell; absent in
    // a plain browser, where the native window controls stay hidden.
    __PROMPTFORGE_DESKTOP__?: boolean;
    // The wry IPC bridge; only present in the desktop shell.
    ipc?: { postMessage(message: string): void };
  }
}

/** The only messages the title bar may send to the native shell. */
const WindowCommand = {
  Drag: "drag",
  Minimize: "minimize",
  ToggleMaximize: "toggle-maximize",
  Close: "close",
} as const;
type WindowCommand = (typeof WindowCommand)[keyof typeof WindowCommand];

/** The IPC envelope: one JSON object naming one window command. */
interface WindowCommandEnvelope {
  readonly command: WindowCommand;
}

/** The native event the shell dispatches when the maximized state changes. */
const MAXIMIZED_EVENT = "promptforge:maximized";

function postWindowCommand(command: WindowCommand): void {
  const ipc = window.ipc;
  // Reached in a plain browser, where the Window menu's native commands
  // have no bridge to carry them; dropping the command beats throwing
  // from a click. In the desktop shell wry always installs the bridge
  // alongside the flag.
  if (!ipc) {
    return;
  }
  const envelope: WindowCommandEnvelope = { command };
  ipc.postMessage(JSON.stringify(envelope));
}

/** Minimizes the window. Shared by the visible control and the Window menu. */
export function minimizeWindow(): void {
  postWindowCommand(WindowCommand.Minimize);
}

/** Toggles between maximized and restored. Shared by the visible control, the drag region double-click, and the Window menu. */
export function toggleWindowMaximize(): void {
  postWindowCommand(WindowCommand.ToggleMaximize);
}

/** Closes the window. Shared by the visible control and the File menu. */
export function closeWindow(): void {
  postWindowCommand(WindowCommand.Close);
}

/** Reads the maximized flag out of the native event, validating the detail. */
function readMaximized(event: Event): boolean | null {
  if (!(event instanceof CustomEvent)) {
    return null;
  }
  const detail: unknown = event.detail;
  if (typeof detail !== "object" || detail === null || !("maximized" in detail)) {
    return null;
  }
  return typeof detail.maximized === "boolean" ? detail.maximized : null;
}

/**
 * Reveals the custom title bar in every mode: the bar carries the
 * application menus, so it must show in a plain browser too. The drag
 * region, the window controls, and the maximized-event listener are wired
 * only inside the desktop shell; in a browser the control cluster is
 * hidden instead, since the commands would have no IPC bridge to reach.
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

  if (window.__PROMPTFORGE_DESKTOP__ !== true) {
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
      postWindowCommand(WindowCommand.Drag);
    }
  };
  drag.addEventListener("pointerdown", onDragPointerDown);
  store.add(toDisposable(() => drag.removeEventListener("pointerdown", onDragPointerDown)));
  const onDragDoubleClick = (): void => postWindowCommand(WindowCommand.ToggleMaximize);
  drag.addEventListener("dblclick", onDragDoubleClick);
  store.add(toDisposable(() => drag.removeEventListener("dblclick", onDragDoubleClick)));

  const onMaximized = (event: Event): void => {
    const maximized = readMaximized(event);
    if (maximized === null) {
      return;
    }
    maximize.setAttribute("aria-label", maximized ? "Restore" : "Maximize");
    maximizeGlyph.toggleAttribute("hidden", maximized);
    restoreGlyph.toggleAttribute("hidden", !maximized);
  };
  window.addEventListener(MAXIMIZED_EVENT, onMaximized);
  store.add(toDisposable(() => window.removeEventListener(MAXIMIZED_EVENT, onMaximized)));
  return store;
}
