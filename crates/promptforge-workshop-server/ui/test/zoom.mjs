// Unit test for the global zoom (src/ui/zoom.ts), its keybindings in
// src/ui/workshop/shortcuts.ts, and its Window menu entries in
// src/ui/window-menu.ts. Bundles the TS modules with esbuild into one
// module graph - so the menu rows, the keydown listener, and the test all
// share one zoom state - with "@tauri-apps/api/window" and
// "@tauri-apps/api/webviewWindow" aliased to the recording stubs in
// test/helpers, and drives them against jsdom built from the real
// index.html. Covers: the zoom math (0.1 steps, clamped to 0.5-2.0, reset
// to 1.0), the browser fallback's CSS zoom application, persistence
// across a reload, corrupt and out-of-range stored values falling back to
// the default, a storage failure leaving the zoom applied and logged, the
// Ctrl+= / Ctrl+Shift+= / Ctrl+- / Ctrl+0 keybindings,
// the Window menu's zoom rows, and the desktop path routing zoom to the
// native webview instead of the CSS fallback. Overlay anchoring at
// non-1.0 zoom is a visual check, deferred out of jsdom scope.
// Run: node test/zoom.mjs
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
      export { installShortcuts } from "./src/ui/workshop/shortcuts.ts";
      export { setupWindowMenus } from "./src/ui/window-menu.ts";
      export {
        getZoom,
        restoreZoom,
        resetZoom,
        zoomIn,
        zoomOut,
        ZOOM_STORAGE_KEY,
      } from "./src/ui/zoom.ts";
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
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
    "@tauri-apps/api/webviewWindow": path.join(uiDir, "helpers", "tauri-webview-stub.mjs"),
  },
});
const bundleCode = bundle.outputFiles[0].text;

// Each import of a fresh data URL is a fresh module instance with its own
// zoom state - the same thing a reload produces. The unique comment
// defeats the module map's URL-keyed cache.
let instanceCounter = 0;
async function freshModule() {
  instanceCounter += 1;
  const code = `${bundleCode}\n// instance ${instanceCounter}`;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Each scenario gets a fresh jsdom (fresh localStorage, fresh DOM) plus a
// fresh module instance. Pass desktop: true to exercise the native
// webview path through the recording stub.
async function scenario({ desktop = false } = {}) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  if (desktop) {
    window.__TAURI_INTERNALS__ = {};
  }
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.Element = window.Element;
  globalThis.HTMLElement = window.HTMLElement;
  globalThis.HTMLInputElement = window.HTMLInputElement;
  globalThis.HTMLTextAreaElement = window.HTMLTextAreaElement;
  globalThis.Node = window.Node;
  const module = await freshModule();
  const press = (key, options = {}) => {
    const event = new window.KeyboardEvent("keydown", {
      key,
      ctrlKey: true,
      cancelable: true,
      ...options,
    });
    window.document.dispatchEvent(event);
    return event;
  };
  const webviewZooms = () => window.__TAURI_WEBVIEW_STUB__?.zooms ?? [];
  return { window, module, press, webviewZooms };
}

// --- Zoom math: 0.1 steps, clamped to 0.5-2.0, reset to 1.0 ----------------

{
  const { window, module } = await scenario();
  check("zoom starts at 100%", module.getZoom() === 1);
  module.zoomIn();
  check("zoomIn steps up by 0.1", module.getZoom() === 1.1);
  module.zoomOut();
  module.zoomOut();
  check("zoomOut steps down by 0.1 without float drift", module.getZoom() === 0.9);
  for (let i = 0; i < 20; i++) module.zoomIn();
  check("zoomIn clamps at 2.0", module.getZoom() === 2);
  for (let i = 0; i < 30; i++) module.zoomOut();
  check("zoomOut clamps at 0.5", module.getZoom() === 0.5);
  module.resetZoom();
  check("resetZoom returns to 100%", module.getZoom() === 1);
  module.zoomIn();
  check(
    "the browser fallback applies CSS zoom to the root element",
    window.document.documentElement.style.zoom === "1.1",
  );
  check(
    "the browser fallback positions body for the overlay workaround",
    window.document.body.style.position === "relative",
  );
}

// --- Persistence: the factor survives a reload ------------------------------

{
  const first = await scenario();
  first.module.zoomIn();
  first.module.zoomIn();
  check(
    "zooming persists the factor to localStorage",
    first.window.localStorage.getItem(first.module.ZOOM_STORAGE_KEY) === "1.2",
  );
  // A reload: a fresh module instance over the same window and storage.
  const reloaded = await freshModule();
  check("a reload boots at the default until restored", reloaded.getZoom() === 1);
  reloaded.restoreZoom();
  check("a reload restores the persisted factor", reloaded.getZoom() === 1.2);
  check(
    "the restore re-applies CSS zoom to the root element",
    first.window.document.documentElement.style.zoom === "1.2",
  );
}

// --- Persistence: corrupt and out-of-range values fall back -----------------

{
  const { window, module } = await scenario();
  for (const bad of ["garbage", "5", "0.1", "NaN"]) {
    window.localStorage.setItem(module.ZOOM_STORAGE_KEY, bad);
    module.restoreZoom();
    check(
      `a corrupt stored value (${JSON.stringify(bad)}) falls back to 100%`,
      module.getZoom() === 1,
    );
  }
}

// --- Persistence: a storage failure does not block the zoom ------------------

{
  const { window, module } = await scenario();
  // jsdom's Storage is a legacy platform object: assigning setItem on the
  // instance stores a "setItem" key instead of overriding the method, so
  // the throwing stub replaces window.localStorage itself.
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      getItem: () => null,
      setItem: () => {
        throw new Error("denied");
      },
    },
  });
  const errors = [];
  const originalError = console.error;
  console.error = (...args) => {
    errors.push(args);
  };
  let escaped = false;
  try {
    module.zoomIn();
  } catch {
    escaped = true;
  }
  console.error = originalError;
  check("a storage failure does not escape the zoom", escaped === false);
  check("the zoom still applies when storage fails", module.getZoom() === 1.1);
  check(
    "the CSS fallback still applies when storage fails",
    window.document.documentElement.style.zoom === "1.1",
  );
  check("a storage failure is logged", errors.length === 1);
}

// --- Keybindings: Ctrl+= / Ctrl+Shift+= / Ctrl+- / Ctrl+0 --------------------

{
  const { module, press } = await scenario();
  const uninstall = module.installShortcuts({});
  let event = press("=");
  check("Ctrl+= zooms in", module.getZoom() === 1.1);
  check("Ctrl+= is consumed", event.defaultPrevented === true);
  // The shifted plus key reports "+" and must beat the handler's Shift
  // early-return.
  event = press("+", { shiftKey: true });
  check("Ctrl+Shift+= zooms in through the shifted + key", module.getZoom() === 1.2);
  check("Ctrl+Shift+= is consumed", event.defaultPrevented === true);
  press("-");
  check("Ctrl+- zooms out", module.getZoom() === 1.1);
  press("0");
  check("Ctrl+0 resets the zoom", module.getZoom() === 1);
  press("=", { altKey: true });
  check("an Alt chord is not a zoom binding", module.getZoom() === 1);
  uninstall.dispose();
  press("=");
  check("disposing uninstalls the keydown listener", module.getZoom() === 1);
}

// --- Window menu: zoom rows dispatch the shared commands ---------------------

{
  const { window, module } = await scenario();
  const commands = module.setupWindowMenus({
    agents: { newAgent: () => {} },
    workshop: {
      toggleWorkshopPanel: () => {},
      openGatewayConfig: () => {},
      openAgentSession: () => {},
    },
  });
  check(
    "the shared command set exposes the zoom commands",
    typeof commands.zoomIn === "function" &&
      typeof commands.zoomOut === "function" &&
      typeof commands.resetZoom === "function",
  );
  const windowButton = window.document.querySelector('[data-menu="window"]');
  const popover = windowButton.nextElementSibling;
  const itemByLabel = (label) =>
    [...popover.querySelectorAll(".window-titlebar__item")].find(
      (item) => item.querySelector(".window-titlebar__item-label").textContent === label,
    );
  windowButton.click();
  const zoomInRow = itemByLabel("Zoom In");
  const zoomOutRow = itemByLabel("Zoom Out");
  const resetRow = itemByLabel("Reset Zoom");
  check(
    "the Window menu lists the zoom commands",
    zoomInRow !== undefined && zoomOutRow !== undefined && resetRow !== undefined,
  );
  check(
    "the zoom rows show their shortcut hints",
    zoomInRow?.querySelector(".window-titlebar__shortcut")?.textContent === "Ctrl+=" &&
      zoomOutRow?.querySelector(".window-titlebar__shortcut")?.textContent === "Ctrl+-" &&
      resetRow?.querySelector(".window-titlebar__shortcut")?.textContent === "Ctrl+0",
  );
  zoomInRow.click();
  check("the menu's Zoom In zooms in", module.getZoom() === 1.1);
  check("running Zoom In closes the menu", popover.hidden === true);
  windowButton.click();
  itemByLabel("Zoom Out").click();
  check("the menu's Zoom Out zooms out", module.getZoom() === 1);
  windowButton.click();
  itemByLabel("Zoom In").click();
  windowButton.click();
  itemByLabel("Reset Zoom").click();
  check("the menu's Reset Zoom returns to 100%", module.getZoom() === 1);
}

// --- Desktop: zoom routes to the native webview, not the CSS fallback --------

{
  const { window, module, webviewZooms } = await scenario({ desktop: true });
  module.zoomIn();
  check("desktop zoom goes to the native webview", webviewZooms().join(",") === "1.1");
  check(
    "desktop zoom leaves the CSS fallback untouched",
    window.document.documentElement.style.zoom === "" &&
      window.document.body.style.position === "",
  );
  check(
    "desktop zoom still persists the factor",
    window.localStorage.getItem(module.ZOOM_STORAGE_KEY) === "1.1",
  );
  const reloaded = await freshModule();
  reloaded.restoreZoom();
  check(
    "a desktop boot restores zoom through the native webview",
    webviewZooms().join(",") === "1.1,1.1",
  );
}

if (failures.length > 0) {
  console.error(`zoom: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("zoom: all assertions passed");
