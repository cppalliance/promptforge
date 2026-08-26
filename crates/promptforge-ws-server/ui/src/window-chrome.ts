// Custom window title bar for the Windows desktop shell. The bar stays
// hidden in a plain browser; only the wry initialization flag reveals it.
// Every control sends a narrow, typed command through the wry IPC bridge -
// the shell (step 9) parses and validates the payload before any native
// window operation runs.

declare global {
  interface Window {
    // Set by the wry initialization script in the desktop shell; absent in
    // a plain browser, where the custom title bar never appears.
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
  // Unreachable in the desktop shell, where wry always installs the bridge
  // alongside the flag that reveals the bar; a missing bridge means the flag
  // was set by hand, and dropping the command beats throwing from a click.
  if (!ipc) {
    return;
  }
  const envelope: WindowCommandEnvelope = { command };
  ipc.postMessage(JSON.stringify(envelope));
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
 * Reveals the custom title bar and wires its window controls when running
 * inside the desktop shell; a no-op in a plain browser, where the bar stays
 * hidden and `window.ipc` is never touched. The menu buttons are inert
 * placeholders here - step 7 wires the popovers.
 */
export function setupWindowChrome(): void {
  const bar = document.querySelector<HTMLElement>(".window-titlebar");
  if (!bar) {
    throw new Error("DOM Error: .window-titlebar not found in the page.");
  }
  if (window.__PROMPTFORGE_DESKTOP__ !== true) {
    return;
  }
  const drag = bar.querySelector<HTMLElement>(".window-titlebar__drag");
  const minimize = bar.querySelector<HTMLButtonElement>('[data-command="minimize"]');
  const maximize = bar.querySelector<HTMLButtonElement>('[data-command="toggle-maximize"]');
  const close = bar.querySelector<HTMLButtonElement>('[data-command="close"]');
  if (!drag || !minimize || !maximize || !close) {
    throw new Error("DOM Error: the title bar is missing a drag region or a window control.");
  }
  const maximizeGlyph = maximize.querySelector<HTMLElement>(".window-titlebar__glyph--maximize");
  const restoreGlyph = maximize.querySelector<HTMLElement>(".window-titlebar__glyph--restore");
  if (!maximizeGlyph || !restoreGlyph) {
    throw new Error("DOM Error: the maximize control is missing its glyphs.");
  }

  bar.hidden = false;

  minimize.addEventListener("click", () => postWindowCommand(WindowCommand.Minimize));
  maximize.addEventListener("click", () => postWindowCommand(WindowCommand.ToggleMaximize));
  close.addEventListener("click", () => postWindowCommand(WindowCommand.Close));

  // Only the empty center drags; the buttons handle their own presses.
  drag.addEventListener("pointerdown", (event) => {
    if (event.button === 0 && event.target === drag) {
      postWindowCommand(WindowCommand.Drag);
    }
  });
  drag.addEventListener("dblclick", () => postWindowCommand(WindowCommand.ToggleMaximize));

  window.addEventListener(MAXIMIZED_EVENT, (event) => {
    const maximized = readMaximized(event);
    if (maximized === null) {
      return;
    }
    maximize.setAttribute("aria-label", maximized ? "Restore" : "Maximize");
    maximizeGlyph.hidden = maximized;
    restoreGlyph.hidden = !maximized;
  });
}
