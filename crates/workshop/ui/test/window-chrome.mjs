// Unit test for the custom window title bar (src/parts/chrome/window-chrome.ts). Bundles
// the TS module with esbuild - with "@tauri-apps/api/window" aliased to the
// recording stub in test/helpers - imports it via a data URL, and drives it
// against jsdom built from the real index.html. Covers: without
// __TAURI_INTERNALS__ the bar is revealed but the control cluster hides and
// no native call is made; a missing bar throws; in the desktop app each
// control calls its window method; the drag region only drags on the
// primary button; double-click toggles maximize; and the maximized state
// read back on resize switches the glyph and aria-label on transitions
// only, with the listener dying at dispose. Also covers the title bar's
// toolbars (Toggle Primary Side Bar on the left, Toggle Agents and the
// settings gear on the right: icon buttons that run their command and
// carry the chord in their tooltip) and the inactive-window class that
// follows the window's blur and focus.
// Run: node test/window-chrome.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { setupWindowChrome } from "./src/parts/chrome/window-chrome.ts";
      export { CommandRegistry } from "@workshop/platform/command-registry";
      export { createKeybindingsRegistry } from "@workshop/platform/keybinding-registry";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module under test imports its colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
  },
});
const code = bundle.outputFiles[0].text;
const { setupWindowChrome, CommandRegistry, createKeybindingsRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

// The toolbars dispatch through a recording command registry and label their
// tooltips from a private keybinding registry, so no global state is shared.
const ran = [];
const commands = new CommandRegistry();
for (const id of [
  "workbench.action.toggleSidebarVisibility",
  "workbench.action.toggleAuxiliaryBar",
  "workbench.action.openSettings",
]) {
  commands.register(id, { run: () => ran.push(id) });
}
const keybindings = createKeybindingsRegistry("windows");
keybindings.registerKeybindingRule({ id: "workbench.action.toggleSidebarVisibility", keybinding: "ctrl+b" });

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets the async maximized sync (a stubbed promise) run to completion.
async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// Each scenario gets a fresh jsdom: setupWindowChrome reads the globals and
// attaches listeners to the DOM it finds at call time.
function scenario({ desktop }) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  if (desktop) {
    window.__TAURI_INTERNALS__ = {};
  }
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.CustomEvent = window.CustomEvent;
  const chrome = setupWindowChrome({ commands, keybindings });
  return {
    window,
    chrome,
    bar: window.document.querySelector(".ws-window-titlebar"),
    stub: () => window.__TAURI_STUB__,
  };
}

// --- Browser mode: bar visible for the menus, native controls hidden --------

{
  const { window, bar } = scenario({ desktop: false });
  check("browser mode reveals the bar", bar.hidden === false);
  const controls = bar.querySelector(".ws-window-titlebar__controls");
  check("browser mode hides the window-control cluster", controls.hidden === true);
  check("browser mode never installs the Tauri internals", !("__TAURI_INTERNALS__" in window));
  check("browser mode makes no native call", window.__TAURI_STUB__ === undefined);
}

// --- Missing markup: the module guards the DOM contract ---------------------

{
  const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  let threw = false;
  try {
    setupWindowChrome();
  } catch {
    threw = true;
  }
  check("a page without the title bar throws", threw);
}

// --- Desktop mode: reveal and native window commands ------------------------

{
  const { window, bar, chrome, stub } = scenario({ desktop: true });
  check("desktop mode reveals the bar", bar.hidden === false);
  check(
    "desktop mode keeps the window-control cluster visible",
    bar.querySelector(".ws-window-titlebar__controls").hidden === false,
  );

  const callsAfterClick = (command) => {
    stub().calls.length = 0;
    bar.querySelector(`[data-command="${command}"]`).click();
    return stub().calls.join(",");
  };
  check("minimize calls the window method", callsAfterClick("minimize") === "minimize");
  check(
    "maximize calls the window method",
    callsAfterClick("toggle-maximize") === "toggle-maximize",
  );
  check("close calls the window method", callsAfterClick("close") === "close");

  const drag = bar.querySelector(".ws-window-titlebar__drag");
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("mousedown", { button: 0, detail: 1, bubbles: true }));
  check("primary press in the empty center starts the drag", stub().calls.join(",") === "drag");
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("mousedown", { button: 2, detail: 1, bubbles: true }));
  check("non-primary press does not drag", stub().calls.length === 0);
  // The OS move loop started by the first press swallows the release, so
  // no dblclick ever reaches the page; the second press (detail 2) is the
  // only signal and must toggle maximize without starting another drag.
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("mousedown", { button: 0, detail: 2, bubbles: true }));
  check("second press toggles maximize instead of dragging", stub().calls.join(",") === "toggle-maximize");
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
  check("a dblclick event alone triggers nothing", stub().calls.length === 0);

  // The glyphs are SVG, so visibility is the hidden *attribute* - an
  // SVGSVGElement has no `hidden` IDL property, and assigning one would
  // only create an inert expando that no stylesheet can see.
  const maximize = bar.querySelector('[data-command="toggle-maximize"]');
  const maximizeGlyph = maximize.querySelector(".ws-window-titlebar__glyph--maximize");
  const restoreGlyph = maximize.querySelector(".ws-window-titlebar__glyph--restore");

  await flush();
  check(
    "boot syncs the maximize glyph from the window state",
    maximize.getAttribute("aria-label") === "Maximize" &&
      !maximizeGlyph.hasAttribute("hidden") &&
      restoreGlyph.hasAttribute("hidden"),
  );

  stub().maximized = true;
  stub().resizeHandlers.forEach((handler) => handler({}));
  await flush();
  check(
    "a resize into maximized switches the label to Restore",
    maximize.getAttribute("aria-label") === "Restore",
  );
  check(
    "a resize into maximized swaps the glyphs",
    maximizeGlyph.hasAttribute("hidden") && !restoreGlyph.hasAttribute("hidden"),
  );

  stub().maximized = false;
  stub().resizeHandlers.forEach((handler) => handler({}));
  await flush();
  check(
    "a resize into restored switches the label back to Maximize",
    maximize.getAttribute("aria-label") === "Maximize",
  );
  check(
    "a resize into restored restores the glyphs",
    !maximizeGlyph.hasAttribute("hidden") && restoreGlyph.hasAttribute("hidden"),
  );

  // The resize listener dies with the chrome: a later resize leaves the
  // control alone.
  chrome.dispose();
  await flush();
  stub().maximized = true;
  stub().resizeHandlers.forEach((handler) => handler({}));
  await flush();
  check(
    "after dispose a resize leaves the control alone",
    maximize.getAttribute("aria-label") === "Maximize" &&
      !maximizeGlyph.hasAttribute("hidden") &&
      restoreGlyph.hasAttribute("hidden"),
  );
}

// --- Toolbars: Toggle Primary Side Bar left; Toggle Agents and the gear right ---

for (const desktop of [false, true]) {
  const mode = desktop ? "desktop" : "browser";
  const { bar, chrome } = scenario({ desktop });

  const left = bar.querySelector(".ws-window-titlebar__left");
  const leftToolbar = left.querySelector(":scope > .ws-window-titlebar__toolbar");
  check(`${mode}: the left region holds a toolbar after the menubar`, leftToolbar !== null && leftToolbar.previousElementSibling === left.querySelector(".ws-window-titlebar__menus"));
  const leftButtons = [...(leftToolbar?.querySelectorAll("button") ?? [])];
  check(
    `${mode}: the left toolbar is exactly Toggle Primary Side Bar`,
    leftButtons.length === 1 &&
      leftButtons[0].dataset.commandId === "workbench.action.toggleSidebarVisibility" &&
      leftButtons[0].getAttribute("aria-label") === "Toggle Primary Side Bar",
  );
  check(`${mode}: the left toolbar button draws an icon`, leftButtons[0]?.querySelector("svg") !== null);
  check(
    `${mode}: the tooltip names the chord`,
    leftButtons[0]?.getAttribute("title") === "Toggle Primary Side Bar (Ctrl+B)",
  );

  const right = bar.querySelector(".ws-window-titlebar__right");
  const rightToolbar = right.querySelector(":scope > .ws-window-titlebar__toolbar");
  check(
    `${mode}: the right region's toolbar sits before the window controls`,
    rightToolbar !== null && rightToolbar.nextElementSibling === right.querySelector(".ws-window-titlebar__controls"),
  );
  const rightButtons = [...(rightToolbar?.querySelectorAll("button") ?? [])];
  check(
    `${mode}: the right toolbar is Toggle Agents then Settings`,
    rightButtons.map((button) => `${button.dataset.commandId}:${button.getAttribute("aria-label")}`).join(",") ===
      "workbench.action.toggleAuxiliaryBar:Toggle Agents,workbench.action.openSettings:Settings",
  );
  check(
    `${mode}: a tooltip without a chord is the bare name`,
    rightButtons[0]?.getAttribute("title") === "Toggle Agents",
  );
  check(`${mode}: every toolbar button is a type=button control with an icon`, [...leftButtons, ...rightButtons].every((button) => button.type === "button" && button.querySelector("svg") !== null));

  ran.length = 0;
  for (const button of [...leftButtons, ...rightButtons]) button.click();
  await flush();
  check(
    `${mode}: each toolbar button runs its command`,
    ran.join(",") ===
      "workbench.action.toggleSidebarVisibility,workbench.action.toggleAuxiliaryBar,workbench.action.openSettings",
  );

  chrome.dispose();
  check(`${mode}: dispose removes the toolbars`, bar.querySelector(".ws-window-titlebar__toolbar") === null);
}

// --- Inactive window: the bar follows the window's blur and focus ---------------

{
  const { window, bar, chrome } = scenario({ desktop: false });
  check("a fresh bar is active", !bar.classList.contains("ws-window-titlebar--inactive"));
  window.dispatchEvent(new window.Event("blur"));
  check("a blurred window marks the bar inactive", bar.classList.contains("ws-window-titlebar--inactive"));
  window.dispatchEvent(new window.Event("focus"));
  check("a refocused window clears the mark", !bar.classList.contains("ws-window-titlebar--inactive"));
  window.dispatchEvent(new window.Event("blur"));
  chrome.dispose();
  window.dispatchEvent(new window.Event("focus"));
  check("after dispose the focus listener is gone", bar.classList.contains("ws-window-titlebar--inactive"));
}

// --- The program icon is 16px in a 35px box ------------------------------------

{
  const { bar } = scenario({ desktop: false });
  const icon = bar.querySelector(".ws-window-titlebar__icon");
  check("the program icon is declared 16px", icon.getAttribute("width") === "16" && icon.getAttribute("height") === "16");
}

if (failures.length > 0) {
  console.error(`window-chrome: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("window-chrome: all assertions passed");
