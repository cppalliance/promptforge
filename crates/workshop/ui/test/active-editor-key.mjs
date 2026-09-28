// Integration test for the activeEditor context key
// (src/parts/layout/zones.ts, bindActiveEditorKey) against the panel
// types the editor, agent, workspace, run, and gateway contributions
// register, plus a synthetic "probe" type. Mounts a real Dockview dock in
// jsdom. The editor and probe panels render through the registry's lazy
// renderer (src/parts/layout/panel-types.ts), so the real editor chunk
// loads and runs its register(), and the probe's chunk resolves only
// when the test releases it; the agent, tree, run, and config panels
// render through an inert stand-in, so their chunks never load.
// Covers: activeEditor holds each active panel's type id - editor,
// agent, tree, run, config, and probe - before its chunk loads; it stays
// the type once the editor chunk and the probe chunk load; it follows
// activation between panels; and it is unset while no panel is active.
// Run: node --test test/active-editor-key.mjs
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
      // The editor chunk's closed-editor tracking reads the stack through
      // its token; the implementation's module scope self-registers it.
      import "./src/parts/editor/closed-editors.ts";
      import "./src/parts/editor/editor.contribution.ts";
      import "./src/parts/agent/agent.contribution.ts";
      import "./src/parts/workspace/workspace.contribution.ts";
      import "./src/parts/run/run.contribution.ts";
      import "./src/parts/gateway/gateway.contribution.ts";
      export { createDockview, themeDark } from "dockview";
      export {
        registerPanelFactory,
        registerPanelType,
        resolvePanelContent,
      } from "@workshop/platform/panel-registry";
      export { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
      export { getService } from "@workshop/platform/service-registry";
      export { bindActiveEditorKey, initZones, openInZone, resetZones } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/parts/layout/panel-types.ts";
      export { EditorPanel } from "./src/parts/editor/editor-panel.ts";
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
  // The modules under test import colocated CSS; strip it - jsdom applies
  // no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM('<!doctype html><html><body><div id="dock"></div></body></html>', {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
});
const { window } = dom;

window.matchMedia =
  window.matchMedia ||
  (() => ({
    matches: false,
    media: "",
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  }));
window.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

// CodeMirror measures text through Range, which jsdom does not lay out;
// zero-rect shims are enough because the test never asserts geometry.
const zeroRect = () => ({
  x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0,
  toJSON: () => ({}),
});
window.Range.prototype.getBoundingClientRect = zeroRect;
window.Range.prototype.getClientRects = () => ({
  length: 0,
  item: () => null,
  [Symbol.iterator]: [][Symbol.iterator],
});
window.HTMLElement.prototype.getClientRects = function getClientRects() {
  return { length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] };
};

globalThis.fetch = async (url) => {
  throw new Error(`unexpected fetch in the active-editor-key test: ${url}`);
};

for (const key of [
  "document",
  "navigator",
  "location",
  "Window",
  "HTMLElement",
  "HTMLInputElement",
  "HTMLTextAreaElement",
  "HTMLButtonElement",
  "Node",
  "Element",
  "Range",
  "KeyboardEvent",
  "MutationObserver",
  "ResizeObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;
globalThis.window = window;
globalThis.document = window.document;

const bundlePath = path.join(os.tmpdir(), "promptforge-active-editor-key-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  registerPanelFactory,
  registerPanelType,
  resolvePanelContent,
  CONTEXT_KEY_SERVICE,
  getService,
  bindActiveEditorKey,
  initZones,
  openInZone,
  resetZones,
  createPanelComponent,
  createPanelTabComponent,
  EditorPanel,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// The probe's feature directory resolves only once the test releases it,
// so the key is observable while the chunk is still loading.
let releaseProbe = () => {};
const probeReleased = new Promise((resolve) => {
  releaseProbe = resolve;
});
const probeFeature = {
  register() {
    registerPanelFactory("probe", () => ({
      element: Object.assign(window.document.createElement("div"), { className: "probe-panel" }),
      init() {},
    }));
  },
};
registerPanelType({
  type: "probe",
  title: "Probe",
  defaultZone: "main",
  load: () => probeReleased.then(() => probeFeature),
});

// The real types whose chunks stay unloaded: an inert renderer stands in
// for the registry's lazy one, so no agent socket, tree fetch, or config
// frame starts in jsdom.
const STAND_INS = ["tree", "agent", "run", "config"];
function createComponent(options) {
  if (STAND_INS.includes(options.name)) {
    return { element: window.document.createElement("div"), init() {} };
  }
  return createPanelComponent(options);
}

const element = window.document.getElementById("dock");
element.className = "ws-dock";
const dock = createDockview(element, {
  createComponent,
  createTabComponent: createPanelTabComponent,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
initZones(dock);
resetZones();
const binding = bindActiveEditorKey(dock);
const context = getService(CONTEXT_KEY_SERVICE);
const activeEditor = () => context.getValue("activeEditor");

check("activeEditor is unset before any panel opens", activeEditor() === undefined);

// --- Each type's own id, before its chunk loads ---------------------------------

const opened = {};
for (const type of STAND_INS) {
  opened[type] = openInZone(type, {});
  check(`activeEditor holds '${type}' while its panel is active`, activeEditor() === type);
}

const editor = openInZone("editor", { untitled: 1 });
check(
  "activeEditor holds 'editor' before the editor chunk loads",
  activeEditor() === "editor" && !(resolvePanelContent(editor.view.content) instanceof EditorPanel),
);
await flush();
check(
  "the real editor chunk loaded and mounted its panel",
  resolvePanelContent(editor.view.content) instanceof EditorPanel,
);
check("activeEditor stays 'editor' once the editor chunk loads", activeEditor() === "editor");

// --- Activation moves the key --------------------------------------------------

opened.tree.api.setActive();
check("activeEditor follows activation to the tree", activeEditor() === "tree");
opened.agent.api.setActive();
check("activeEditor follows activation to the agent session", activeEditor() === "agent");
editor.api.setActive();
check("activeEditor follows activation back to the editor", activeEditor() === "editor");

// --- A synthetic type, before and after its chunk loads -------------------------

const probe = openInZone("probe", {});
const probeMounted = () => probe.view.content.element.querySelector(".probe-panel") !== null;
check("activeEditor holds 'probe' before its chunk loads", activeEditor() === "probe" && !probeMounted());
releaseProbe();
await flush();
check("the probe chunk loaded and mounted its panel", probeMounted());
check("activeEditor stays 'probe' once its chunk loads", activeEditor() === "probe");

// --- No active panel -------------------------------------------------------------

for (const panel of [...dock.panels]) {
  dock.removePanel(panel);
}
await flush();
check(
  "activeEditor is unset with no active panel",
  dock.activePanel === undefined && activeEditor() === undefined,
);
binding.dispose();

if (failures.length > 0) {
  console.error(`active-editor-key: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("active-editor-key: all assertions passed");
process.exit(0);
