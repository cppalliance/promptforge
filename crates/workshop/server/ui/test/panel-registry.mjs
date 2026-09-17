// Unit test for the panel registry (src/services/panel-registry.ts) and the
// lazy panel seam in panel-types.ts: panel types are declared with import
// thunks, the feature chunk loads on first activation, the directory's
// register() runs exactly once no matter how many panels open, and the
// real panel swaps into the dockview renderer element when the chunk
// resolves. Bundles the modules with esbuild (the built-in thunks ride
// along but are never triggered) and drives the registry against jsdom
// with a synthetic lazy feature (test/helpers/lazy-feature.mjs). Covers:
// the four built-in panel types' metadata (zone affinity, title, tab
// renderer), isPanelType narrowing, the unknown-name placeholder, lazy
// mount with init params forwarded, register-once semantics, disposal
// reaching the real panel, and the registration outliving the panel.
// Run: node test/panel-registry.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

// The lucide icon module (pulled in by the built-in thunks' graph)
// serializes SVGs through the document at import time, so the globals
// must exist before the bundle loads.
const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.CustomEvent = window.CustomEvent;
globalThis.Event = window.Event;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import { registerPanelType, registerPanelFactory } from "./src/services/panel-registry.ts";
      // The synthetic lazy feature, registered from inside the bundle so
      // its register() shares this module graph's registry instance (the
      // helper receives registerPanelFactory through a global: a static
      // import of the TS source would give it a second registry copy).
      globalThis.__testRegisterPanelFactory = registerPanelFactory;
      registerPanelType({
        type: "fake",
        defaultZone: "main",
        title: "Fake",
        tabComponent: undefined,
        load: () => import("./test/helpers/lazy-feature.mjs"),
      });
      export {
        registerPanelType,
        isPanelType,
        panelTypeEntry,
        PERMANENT_TAB,
        AGENT_TAB,
      } from "./src/services/panel-registry.ts";
      export { createPanelComponent } from "./src/ui/layout/panel-types.ts";
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
});

// The bundle's graph includes dockview and shiki through the built-in
// thunks, so it is far too large for a data URL; import from a temp file.
const bundlePath = path.join(os.tmpdir(), "panel-registry-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  registerPanelType,
  isPanelType,
  panelTypeEntry,
  PERMANENT_TAB,
  AGENT_TAB,
  createPanelComponent,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 10; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- The built-in panel types ------------------------------------------------

check(
  "the tree anchors the left zone with the permanent tab",
  panelTypeEntry("tree")?.defaultZone === "left" &&
    panelTypeEntry("tree")?.title === "Workshop" &&
    panelTypeEntry("tree")?.tabComponent === PERMANENT_TAB,
);
check(
  "the editor opens in the main zone with the default tab",
  panelTypeEntry("editor")?.defaultZone === "main" &&
    panelTypeEntry("editor")?.title === "Editor" &&
    panelTypeEntry("editor")?.tabComponent === undefined,
);
check(
  "the gateway config opens in the main zone",
  panelTypeEntry("config")?.defaultZone === "main" &&
    panelTypeEntry("config")?.title === "Gateway Config",
);
check(
  "the agent session opens in the right zone with its own tab",
  panelTypeEntry("agent")?.defaultZone === "right" &&
    panelTypeEntry("agent")?.title === "Agent Session" &&
    panelTypeEntry("agent")?.tabComponent === AGENT_TAB,
);
check("isPanelType narrows registered names", isPanelType("editor") && !isPanelType("nope"));

// --- Unknown names render a placeholder ---------------------------------------

const unknown = createPanelComponent({ id: "x", name: "nope" });
check(
  "an unknown component name renders a labelled placeholder",
  unknown.element.textContent === "Unknown panel: nope",
);

// --- A lazy panel type loads on first activation -------------------------------
// (The "fake" type was registered inside the bundle, above.)

check("a registered type narrows", isPanelType("fake"));

const renderer = createPanelComponent({ id: "fake", name: "fake" });
check("the lazy renderer mounts nothing before the chunk resolves", renderer.element.childElementCount === 0);
renderer.init({ params: { a: 1 }, api: {} });
await flush();

const inner = renderer.element.querySelector(".fake-lazy-panel");
check("the real panel swaps in once the chunk resolves", inner !== null);
check(
  "the dockview init params reach the real panel",
  inner?.dataset.params === JSON.stringify({ a: 1 }),
);
check("the feature's register() ran", globalThis.__lazyFeatureRegisters === 1);

// A second panel of the same type reuses the registration.
const second = createPanelComponent({ id: "fake2", name: "fake" });
second.init({ params: {}, api: {} });
await flush();
check("the second panel mounts from the same chunk", !!second.element.querySelector(".fake-lazy-panel"));
check("register() ran exactly once across panels", globalThis.__lazyFeatureRegisters === 1);

// Disposal reaches the real panel but not the directory's registration:
// commands, keybindings, and factories live for the page, not the panel.
renderer.dispose();
check("disposing the wrapper disposes the real panel", globalThis.__lazyFeatureDisposes === 1);
check(
  "the registration outlives the panel",
  globalThis.__lazyFeatureRegisterDisposed === undefined,
);

if (failures.length > 0) {
  console.error(`panel-registry: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("panel-registry: all assertions passed");
process.exit(0);
