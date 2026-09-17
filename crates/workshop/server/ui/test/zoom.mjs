// Unit test for the global zoom (src/ui/chrome/zoom.ts) and its menu
// rows, registered by the chrome contribution
// (src/ui/chrome/chrome.contribution.ts) into View > Appearance. Bundles
// the TS modules with esbuild into one
// module graph - so the menu rows and the test all
// share one zoom state - with "@tauri-apps/api/window" and
// "@tauri-apps/api/webviewWindow" aliased to recording stubs in
// test/helpers, and drives them against jsdom built from the real
// index.html. Covers: the zoom math (0.1 steps, clamped to 0.5-2.0, reset
// to 1.0), the browser fallback's CSS zoom application, the write-through
// to the UI-state adapter's user bucket and the restore from it across a
// reload, corrupt and out-of-range stored values falling back to the
// default, a failing writer leaving the zoom applied and logged, the
// Appearance flyout's zoom rows with their shortcut hints, and the
// desktop path routing zoom to the native webview. The Ctrl+= /
// Ctrl+Shift+= / Ctrl+- / Ctrl+NumPad0 / Ctrl+0 keybinding assertions
// live in test/gateway-config-menu.mjs with the dispatcher.
// Overlay anchoring at non-1.0 zoom is a visual check,
// deferred out of jsdom scope.
// Run: node test/zoom.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/ui/chrome/chrome.contribution.ts";
      export { Menu } from "./src/ui/menu/menu.ts";
      export {
        getZoom,
        persistZoom,
        restoreZoom,
        resetZoom,
        zoomIn,
        zoomOut,
      } from "./src/ui/chrome/zoom.ts";
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

// The user-bucket key the composition root binds the zoom to.
const KEY = "zoom";

// Binds a module instance's zoom writer to a fake adapter the way main.ts
// binds the live one; the fake's `sets` records every write.
function bindStorage(module, initial = {}) {
  const storage = createFakeUiStorage(initial);
  module.persistZoom((value) => storage.set("user", KEY, value));
  return storage;
}

// Each scenario gets a fresh jsdom (fresh DOM) plus a fresh module
// instance. Pass desktop: true to exercise the native webview path
// through the recording stub.
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
  const webviewZooms = () => window.__TAURI_WEBVIEW_STUB__?.zooms ?? [];
  return { window, module, webviewZooms };
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
  const storage = bindStorage(first.module);
  first.module.zoomIn();
  first.module.zoomIn();
  check(
    "each zoom step writes the bare factor to the user bucket",
    storage.sets.length === 2 &&
      storage.sets.every((entry) => entry.bucket === "user" && entry.key === KEY) &&
      storage.sets.map((entry) => entry.value).join(",") === "1.1,1.2",
  );
  // A reload: a fresh module instance over the same window, seeded from
  // the value the last write stored.
  const reloaded = await freshModule();
  check("a reload boots at the default until restored", reloaded.getZoom() === 1);
  const reloadedStorage = bindStorage(reloaded, { user: { [KEY]: storage.get("user", KEY) } });
  reloaded.restoreZoom(reloadedStorage.get("user", KEY));
  check("a reload restores the persisted factor", reloaded.getZoom() === 1.2);
  check(
    "the restore re-applies CSS zoom to the root element",
    first.window.document.documentElement.style.zoom === "1.2",
  );
  check("the restore does not echo the factor back to the writer", reloadedStorage.sets.length === 0);
}

// --- Persistence: the writer installed with persistZoom is replaceable -----

{
  const { module } = await scenario();
  const first = bindStorage(module);
  const disposable = module.persistZoom(() => {
    throw new Error("must not be called after dispose");
  });
  disposable.dispose();
  module.zoomIn();
  check(
    "disposing an installed writer restores the no-op, not an earlier writer",
    first.sets.length === 0 && module.getZoom() === 1.1,
  );
}

// --- Persistence: corrupt and out-of-range values fall back -----------------

{
  const { module } = await scenario();
  for (const bad of ["garbage", "1.2", 5, 0.1, Number.NaN, null, undefined, { factor: 1.2 }, [1.2]]) {
    module.restoreZoom(bad);
    check(
      `a corrupt stored value (${String(bad)}) falls back to 100%`,
      module.getZoom() === 1,
    );
  }
  module.restoreZoom(0.5);
  check("the lower bound restores", module.getZoom() === 0.5);
  module.restoreZoom(2);
  check("the upper bound restores", module.getZoom() === 2);
}

// --- Persistence: a failing writer does not block the zoom -------------------

{
  const { window, module } = await scenario();
  module.persistZoom(() => {
    throw new Error("denied");
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
  check("a writer failure does not escape the zoom", escaped === false);
  check("the zoom still applies when the writer fails", module.getZoom() === 1.1);
  check(
    "the CSS fallback still applies when the writer fails",
    window.document.documentElement.style.zoom === "1.1",
  );
  check("a writer failure is logged", errors.length === 1);
}

// --- Appearance flyout: the zoom rows dispatch the registered actions --------

{
  const { window, module } = await scenario();
  const anchor = window.document.createElement("button");
  anchor.type = "button";
  window.document.body.appendChild(anchor);
  const menu = new module.Menu();
  const popover = () =>
    [...window.document.querySelectorAll(".ws-window-titlebar__popover")].find((el) => !el.hidden);
  const rowByLabel = (label) =>
    [...popover().querySelectorAll(":scope > .ws-window-titlebar__item")].find(
      (item) => item.querySelector(".ws-window-titlebar__item-label").textContent === label,
    );
  menu.open("menubar/view/appearance", anchor);
  const zoomInRow = rowByLabel("Zoom In");
  const zoomOutRow = rowByLabel("Zoom Out");
  const resetRow = rowByLabel("Reset Zoom");
  check(
    "the Appearance flyout lists the zoom rows",
    zoomInRow !== undefined && zoomOutRow !== undefined && resetRow !== undefined,
  );
  check(
    "the zoom rows show their shortcut hints",
    zoomInRow?.querySelector(".ws-window-titlebar__shortcut")?.textContent === "Ctrl+=" &&
      zoomOutRow?.querySelector(".ws-window-titlebar__shortcut")?.textContent === "Ctrl+-" &&
      resetRow?.querySelector(".ws-window-titlebar__shortcut")?.textContent === "Ctrl+NumPad0",
  );
  zoomInRow.click();
  check("the menu's Zoom In zooms in", module.getZoom() === 1.1);
  check("running Zoom In closes the menu", popover() === undefined);
  menu.open("menubar/view/appearance", anchor);
  rowByLabel("Zoom Out").click();
  check("the menu's Zoom Out zooms out", module.getZoom() === 1);
  menu.open("menubar/view/appearance", anchor);
  rowByLabel("Zoom In").click();
  menu.open("menubar/view/appearance", anchor);
  rowByLabel("Reset Zoom").click();
  check("the menu's Reset Zoom returns to 100%", module.getZoom() === 1);
  menu.dispose();
}

// --- Desktop: viewport-aware zoom routes through the native webview -----------

{
  const { window, module, webviewZooms } = await scenario({ desktop: true });
  const storage = bindStorage(module);
  module.zoomIn();
  check("desktop zoom goes to the native webview", webviewZooms().join(",") === "1.1");
  check(
    "desktop zoom leaves CSS zoom untouched",
    window.document.documentElement.style.zoom === "" &&
      window.document.body.style.position === "",
  );
  check(
    "desktop zoom still persists the factor",
    storage.sets.length === 1 && storage.get("user", KEY) === 1.1,
  );
  const reloaded = await freshModule();
  reloaded.restoreZoom(storage.get("user", KEY));
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
