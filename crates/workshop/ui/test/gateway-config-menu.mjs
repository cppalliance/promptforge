// Unit test for the step-19 feature contributions: chrome, layout,
// status, agent, and gateway (src/parts/<feature>/<feature>.contribution.ts).
// Bundles the five contribution modules with esbuild - the Tauri APIs
// aliased to the recording stubs in test/helpers - and drives them
// through the shared registries against jsdom with a recording fake
// dock. Covers: the catalog wiring (titles, menu groups, chord labels,
// palette rows); File > Preferences > Settings opening the Gateway
// Config panel, with a second activation focusing the existing panel;
// the zoom chords (Ctrl+=, Ctrl+Shift+=, Ctrl+-, Ctrl+NumPad0, Ctrl+0)
// dispatching through the keybinding dispatcher to the real zoom
// functions; F11 toggling native fullscreen; Close Window reaching the
// native window; About opening the dialog; New Agents Window opening a
// keyed agent panel; and the Primary Side Bar / Secondary Side Bar /
// Status Bar toggles flipping their targets and context keys.
// Run: node --test test/gateway-config-menu.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/parts/chrome/chrome.contribution.ts";
      import "./src/parts/layout/layout.contribution.ts";
      import "./src/parts/status/status.contribution.ts";
      import "./src/parts/agent/agent.contribution.ts";
      import "./src/parts/gateway/gateway.contribution.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus } from "./src/services/menu-registry.ts";
      export { KeybindingsRegistry } from "./src/services/keybinding-registry.ts";
      export { CONTEXT_KEY_SERVICE } from "./src/services/context-key-service.ts";
      export { getService, registerService } from "./src/services/service-registry.ts";
      export { STATUS_BAR } from "./src/parts/status/status-bar.ts";
      export { Menu } from "./src/parts/menu/menu.ts";
      export { KeybindingDispatcher } from "./src/parts/layout/keybinding-dispatcher.ts";
      export { initZones } from "./src/parts/layout/zones.ts";
      export { getZoom } from "./src/parts/chrome/zoom.ts";
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
  // The modules under test import colocated CSS; the test drives only
  // the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/plugin-dialog": path.join(uiDir, "helpers", "tauri-dialog-stub.mjs"),
    "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
    "@tauri-apps/api/webviewWindow": path.join(uiDir, "helpers", "tauri-webview-stub.mjs"),
  },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;

for (const key of [
  "document",
  "navigator",
  "HTMLElement",
  "HTMLButtonElement",
  "Node",
  "Element",
  "Event",
  "CustomEvent",
  "MutationObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;
// Events dispatched into the jsdom document must be jsdom-realm instances.
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;
// New Agents Window keys its panel by a random UUID.
if (typeof window.crypto?.randomUUID !== "function") {
  let serial = 0;
  Object.defineProperty(window, "crypto", {
    configurable: true,
    value: { randomUUID: () => `uuid-${(serial += 1)}` },
  });
}

// The contributions register at module scope; a malformed descriptor
// reports through console.error, so spy on it across the bundle import.
const consoleErrors = [];
const realConsoleError = console.error;
console.error = (...args) => {
  consoleErrors.push(args.join(" "));
};

const bundlePath = path.join(os.tmpdir(), "promptforge-step19-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  Commands,
  Menus,
  KeybindingsRegistry,
  CONTEXT_KEY_SERVICE,
  getService,
  registerService,
  STATUS_BAR,
  Menu,
  KeybindingDispatcher,
  initZones,
  getZoom,
} = await import(pathToFileURL(bundlePath).href);
console.error = realConsoleError;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

check("no contribution reported a malformed descriptor", consoleErrors.length === 0);

// --- Shared fakes ------------------------------------------------------------

// The status bar toggle targets the composition root's StatusBar; a
// recording stand-in takes its slot and, like the real bar's setVisible,
// mirrors the outcome into the statusBarVisible context key.
let barVisible = true;
registerService(STATUS_BAR, () => ({
  get isVisible() {
    return barVisible;
  },
  setVisible(visible) {
    barVisible = visible;
    getService(CONTEXT_KEY_SERVICE).createKey("statusBarVisible", true).set(visible);
  },
}));

// A recording fake dock: added panels, activations, removals, and the
// per-group visibility the Secondary Side Bar toggle drives.
const addedPanels = [];
const activated = [];
const removed = [];
const groups = new Map();
function makeGroup(id) {
  const api = {
    isVisible: true,
    setVisible(visible) {
      api.isVisible = visible;
    },
  };
  const group = { id, api };
  groups.set(id, group);
  return group;
}
initZones({
  panels: [],
  groups: [],
  getPanel: (id) => addedPanels.find((panel) => panel.id === id),
  getGroup: (id) => groups.get(id),
  addPanel: (opts) => {
    const panel = {
      id: opts.id,
      params: opts.params,
      title: opts.title,
      group: makeGroup(`g-${opts.id}`),
      api: { setActive() {
        activated.push(opts.id);
      } },
      view: { content: {} },
    };
    addedPanels.push(panel);
    return panel;
  },
  removePanel: (panel) => {
    removed.push(panel.id);
    const index = addedPanels.indexOf(panel);
    if (index !== -1) addedPanels.splice(index, 1);
  },
  onDidMovePanel: () => ({ dispose() {} }),
  onDidRemovePanel: () => ({ dispose() {} }),
  onWillMutateLayout: () => ({ dispose() {} }),
  onDidMutateLayout: () => ({ dispose() {} }),
  onDidLayoutChange: () => ({ dispose() {} }),
});

const contextKeys = getService(CONTEXT_KEY_SERVICE);

// --- Catalog wiring ----------------------------------------------------------

{
  const prefRows = Menus.getMenuItems("menubar/file/preferences");
  check(
    "Preferences holds the Settings row in 1_settings",
    prefRows.length === 1 &&
      prefRows[0].command === "workbench.action.openSettings" &&
      prefRows[0].group === "1_settings",
  );

  const titles = (rows) => rows.map((row) => Commands.lookup(row.command)?.title);
  const appearance = titles(Menus.getMenuItems("menubar/view/appearance"));
  check(
    "Appearance carries the step-19 rows in group order",
    appearance.join(",") ===
      "Full Screen,Primary Side Bar,Secondary Side Bar,Status Bar,Zoom In,Zoom Out,Reset Zoom",
  );
  const file = titles(Menus.getMenuItems("menubar/file"));
  check(
    "File carries New Agents Window and Close Window",
    file.includes("New Agents Window") && file.includes("Close Window"),
  );
  const help = titles(Menus.getMenuItems("menubar/help"));
  check("Help carries About", help.includes("About"));

  const palette = new Set(Menus.getMenuItems("commandPalette").map((row) => row.command));
  const f1Ids = [
    "workbench.action.toggleFullScreen",
    "workbench.action.closeWindow",
    "workbench.action.zoomIn",
    "workbench.action.zoomOut",
    "workbench.action.zoomReset",
    "workbench.action.showAboutDialog",
    "workbench.view.explorer",
    "workbench.action.toggleSidebarVisibility",
    "workbench.action.toggleAuxiliaryBar",
    "workbench.action.toggleStatusbarVisibility",
    "workbench.action.newAgentsWindow",
    "workbench.action.openSettings",
  ];
  check(
    "every step-19 action reaches the palette",
    f1Ids.every((id) => palette.has(id)),
  );

  const label = (id) => KeybindingsRegistry.lookupKeybinding(id)?.getLabel();
  check("Settings shows Ctrl+,", label("workbench.action.openSettings") === "Ctrl+,");
  check("Reset Zoom's label is the NumPad0 rule", label("workbench.action.zoomReset") === "Ctrl+NumPad0");
  check("Full Screen shows F11", label("workbench.action.toggleFullScreen") === "F11");
  check("New Agents Window shows Ctrl+Alt+N", label("workbench.action.newAgentsWindow") === "Ctrl+Alt+N");
  check("Explorer shows Ctrl+Shift+E", label("workbench.view.explorer") === "Ctrl+Shift+E");
  check("Secondary Side Bar shows Ctrl+Alt+B", label("workbench.action.toggleAuxiliaryBar") === "Ctrl+Alt+B");
}

// --- Preferences > Settings opens the Gateway Config panel -------------------

const menu = new Menu();
const anchor = window.document.createElement("button");
anchor.type = "button";
window.document.body.appendChild(anchor);
// The menubar's submenu declarations land with the menubar contribution
// (plan step 20); the test declares the one flyout it drives.
Menus.appendMenuItem("menubar/file", {
  submenu: "menubar/file/preferences",
  title: "Preferences",
  group: "5_prefs",
});

function popovers() {
  return [...window.document.querySelectorAll(".ws-window-titlebar__popover")].filter(
    (el) => !el.hidden,
  );
}
function rowByLabel(popover, label) {
  return [...popover.querySelectorAll(":scope > .ws-window-titlebar__item")].find(
    (row) => row.querySelector(".ws-window-titlebar__item-label")?.textContent === label,
  );
}
function openPreferences() {
  menu.open("menubar/file", anchor);
  const filePopover = popovers().at(-1);
  rowByLabel(filePopover, "Preferences").dispatchEvent(
    new window.Event("pointerenter", { bubbles: false }),
  );
  return popovers().at(-1);
}

{
  const flyout = openPreferences();
  const settings = rowByLabel(flyout, "Settings");
  check("the Preferences flyout lists Settings", settings !== undefined);
  check(
    "the Settings row shows its chord",
    settings?.querySelector(".ws-window-titlebar__shortcut")?.textContent === "Ctrl+,",
  );
  settings.click();
  await flush();
  const configPanels = addedPanels.filter((panel) => panel.id === "config");
  check("Settings opens the Gateway Config panel", configPanels.length === 1);
  check("the panel opens titled Gateway Config", configPanels[0]?.title === "Gateway Config");
  check("activating a row closes the menu", popovers().length === 0);

  const again = openPreferences();
  rowByLabel(again, "Settings").click();
  await flush();
  check(
    "a second activation focuses the existing panel",
    addedPanels.filter((panel) => panel.id === "config").length === 1 &&
      activated.includes("config"),
  );
}

// --- Zoom chords through the keybinding dispatcher -----------------------------

const dispatcher = new KeybindingDispatcher({
  status: { show() {}, showError() {}, clear() {} },
});
function press(key, init = {}) {
  const event = new window.KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  window.document.dispatchEvent(event);
  return event;
}

{
  check("zoom starts at 100%", getZoom() === 1);
  const equal = press("=", { code: "Equal", ctrlKey: true });
  check("Ctrl+= dispatches zoomIn", getZoom() === 1.1);
  check("Ctrl+= is consumed", equal.defaultPrevented === true);
  const shifted = press("+", { code: "Equal", ctrlKey: true, shiftKey: true });
  check("Ctrl+Shift+= dispatches zoomIn through the shifted + key", getZoom() === 1.2);
  check("Ctrl+Shift+= is consumed", shifted.defaultPrevented === true);
  press("-", { code: "Minus", ctrlKey: true });
  check("Ctrl+- dispatches zoomOut", getZoom() === 1.1);
  press("0", { code: "Numpad0", ctrlKey: true });
  check("Ctrl+NumPad0 dispatches zoomReset", getZoom() === 1);
  press("=", { code: "Equal", ctrlKey: true });
  press("0", { code: "Digit0", ctrlKey: true });
  check("Ctrl+0 is the second zoomReset rule", getZoom() === 1);
  check(
    "the browser fallback applied the CSS zoom",
    window.document.documentElement.style.zoom !== "",
  );
}

// --- Full Screen and Close Window reach the native window ----------------------

window.__TAURI_INTERNALS__ = {};
{
  press("F11", { code: "F11" });
  await flush();
  check("F11 enters native fullscreen", window.__TAURI_STUB__?.fullscreen === true);
  press("F11", { code: "F11" });
  await flush();
  check("F11 again leaves native fullscreen", window.__TAURI_STUB__?.fullscreen === false);

  await Commands.execute("workbench.action.closeWindow");
  await flush();
  check(
    "Close Window closes the native window",
    window.__TAURI_STUB__?.calls.includes("close"),
  );
}

// --- About, New Agents Window, and the three visibility toggles -----------------

{
  await Commands.execute("workbench.action.showAboutDialog");
  const dialog = window.document.querySelector(".ws-about-dialog");
  check("About opens the dialog", dialog !== null);
  dialog?.querySelector(".ws-about-dialog__close")?.click();
  check(
    "the dialog's Close dismisses it",
    window.document.querySelector(".ws-about-dialog") === null,
  );

  await Commands.execute("workbench.action.newAgentsWindow");
  const agentPanels = addedPanels.filter((panel) => panel.id.startsWith("agent:"));
  check(
    "New Agents Window opens a keyed agent panel",
    agentPanels.length === 1 && typeof agentPanels[0]?.params?.instance === "string",
  );

  await Commands.execute("workbench.action.toggleAuxiliaryBar");
  const agentGroup = agentPanels[0]?.group;
  check(
    "Secondary Side Bar hides the agent zone's group",
    agentGroup?.api.isVisible === false &&
      contextKeys.getValue("auxiliaryBarVisible") === false,
  );
  await Commands.execute("workbench.action.toggleAuxiliaryBar");
  check(
    "Secondary Side Bar shows the group again",
    agentGroup?.api.isVisible === true &&
      contextKeys.getValue("auxiliaryBarVisible") === true,
  );

  await Commands.execute("workbench.action.toggleSidebarVisibility");
  check(
    "Primary Side Bar opens the tree and sets its key",
    addedPanels.some((panel) => panel.id === "tree") &&
      contextKeys.getValue("sideBarVisible") === true,
  );
  await Commands.execute("workbench.action.toggleSidebarVisibility");
  check(
    "Primary Side Bar again removes the tree and clears its key",
    removed.includes("tree") && contextKeys.getValue("sideBarVisible") === false,
  );

  await Commands.execute("workbench.action.toggleStatusbarVisibility");
  check(
    "Status Bar hides the bar and clears its key",
    barVisible === false && contextKeys.getValue("statusBarVisible") === false,
  );
  await Commands.execute("workbench.action.toggleStatusbarVisibility");
  check(
    "Status Bar again shows the bar",
    barVisible === true && contextKeys.getValue("statusBarVisible") === true,
  );
}

dispatcher.dispose();
menu.dispose();

if (failures.length > 0) {
  console.error(`gateway-config-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("gateway-config-menu: all assertions passed");
