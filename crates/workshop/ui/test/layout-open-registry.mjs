// Integration test for the open panel registry (@workshop/platform's
// panel-registry.ts) driven through the zone registry
// (src/parts/layout/zones.ts), layout persistence, the layout boot
// decision (src/parts/layout/layout-boot.ts), and the layout
// contribution's Secondary Side Bar toggle. Bundles the layout core
// without any feature contribution, mounts a real Dockview dock in
// jsdom, and registers synthetic panel types the core has never heard of.
// Covers: the core declares no panel type of its own; a registered type
// opens in its default zone with its static title; the `instance` param
// keys side-by-side instances and reopening one reveals it; a custom
// panelId and title function govern identity and the tab title; a
// `closable: false` type opens and keeps the flag; ids holding a Windows
// drive colon still resolve their type; save-then-restore brings every
// panel back with its params, title, and zone; a duplicate registration
// throws; and a registered layout policy over the synthetic types (left
// anchor "pinned", right anchor "probe") seeds the default layout, gets
// a lost anchor back after a restore without seeding, and gives the
// Secondary Side Bar toggle its right anchor to open when the right zone
// was never built - the layout core names no feature.
// Run: node --test test/layout-open-registry.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      // The dock lifecycle reads the closed-editor stack through its token;
      // the implementation's module scope self-registers the empty default.
      import "./src/parts/editor/closed-editors.ts";
      // The Secondary Side Bar toggle registers from the layout core's own
      // contribution.
      import "./src/parts/layout/layout.contribution.ts";
      export { createDockview, themeDark } from "dockview";
      export {
        registerPanelType,
        registerPanelFactory,
        isPanelType,
        panelTypeEntry,
      } from "@workshop/platform/panel-registry";
      export {
        initZones,
        openInZone,
        panelIdFor,
        resetZones,
        setZoneOverride,
        zoneOfPanel,
      } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/parts/layout/panel-types.ts";
      export { restoreLayout, buildLayoutEnvelope } from "./src/parts/layout/layout-persistence.ts";
      export { applyLayoutOrDefault } from "./src/parts/layout/layout-boot.ts";
      export { Commands } from "@workshop/platform/command-registry";
      export { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
      export { getService, registerService } from "@workshop/platform/service-registry";
      export { LAYOUT_POLICY } from "./src/services/layout-policy.ts";
      export { ZONE_STATE } from "./src/services/zone-state-service.ts";
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
  loader: { ".css": "empty" },
});

const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
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

globalThis.fetch = async (url) => {
  throw new Error(`unexpected fetch in the layout-open-registry test: ${url}`);
};

for (const key of [
  "document",
  "navigator",
  "location",
  "Window",
  "HTMLElement",
  "Node",
  "Element",
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

const bundlePath = path.join(os.tmpdir(), "layout-open-registry-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  registerPanelType,
  registerPanelFactory,
  isPanelType,
  panelTypeEntry,
  initZones,
  openInZone,
  panelIdFor,
  resetZones,
  setZoneOverride,
  zoneOfPanel,
  createPanelComponent,
  createPanelTabComponent,
  restoreLayout,
  buildLayoutEnvelope,
  applyLayoutOrDefault,
  Commands,
  CONTEXT_KEY_SERVICE,
  getService,
  registerService,
  LAYOUT_POLICY,
  ZONE_STATE,
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

function createDock(element) {
  element.className = "ws-dock";
  window.document.body.appendChild(element);
  return createDockview(element, {
    createComponent: createPanelComponent,
    createTabComponent: createPanelTabComponent,
    theme: themeDark,
    disableFloatingGroups: true,
    hideBorders: true,
    locked: false,
    noPanelsOverlay: "emptyGroup",
  });
}

/** The probe content a panel element mounted, once its chunk resolved. */
function probeContent(panel) {
  return panel.view.content.element.querySelector(".probe-panel");
}

// --- The core knows no panel type ---------------------------------------------

check(
  "the core declares no panel type of its own",
  !["tree", "editor", "config", "agent", "run"].some((type) => isPanelType(type)),
);

// --- Synthetic types, registered the way a feature contribution does ---------

// One lazy "feature directory" serving all three synthetic types.
const probeFeature = {
  register() {
    for (const type of ["probe", "custom", "pinned"]) {
      registerPanelFactory(type, () => ({
        element: Object.assign(window.document.createElement("div"), { className: "probe-panel" }),
        init(parameters) {
          this.element.dataset.type = type;
          this.element.dataset.params = JSON.stringify(parameters.params);
        },
      }));
    }
  },
};
const load = () => Promise.resolve(probeFeature);

registerPanelType({ type: "probe", title: "Probe", defaultZone: "right", load });
registerPanelType({
  type: "custom",
  title: (params) => `Custom ${params.name}`,
  defaultZone: "main",
  panelId: (params) => `custom:${params.name}`,
  load,
});
registerPanelType({ type: "pinned", title: "Pinned", defaultZone: "left", closable: false, load });

check("a registered synthetic type narrows", isPanelType("probe") && isPanelType("custom"));

let duplicateThrew = false;
try {
  registerPanelType({ type: "probe", title: "Probe again", defaultZone: "main", load });
} catch {
  duplicateThrew = true;
}
check("registering a duplicate type throws", duplicateThrew);
check("the duplicate left the first registration in place", panelTypeEntry("probe")?.title === "Probe");

// --- The identity rule ---------------------------------------------------------

check("a type without panelId or instance is a singleton id", panelIdFor("probe", {}) === "probe");
check("a string instance keys the id as type:instance", panelIdFor("probe", { instance: "a" }) === "probe:a");
check("a non-string instance is ignored", panelIdFor("probe", { instance: 7 }) === "probe");
check(
  "a custom panelId wins over the instance rule",
  panelIdFor("custom", { name: "x", instance: "ignored" }) === "custom:x",
);

// --- Opening, instance keying, titles, zones -----------------------------------

const dock = createDock(window.document.getElementById("dock"));
initZones(dock);
resetZones();

const single = openInZone("probe", {});
const first = openInZone("probe", { instance: "a" });
const second = openInZone("probe", { instance: "b" });
const custom = openInZone("custom", { name: "x" });
const pinned = openInZone("pinned", {});
await flush();

check("the singleton probe opens under its bare type id", single.id === "probe");
check("instances open side by side", first.id === "probe:a" && second.id === "probe:b" && first !== second);
check(
  "every probe opens in its default zone",
  [single, first, second].every((panel) => zoneOfPanel(panel) === "right"),
);
check("a static title becomes the tab title", single.title === "Probe" && first.title === "Probe");
check("a custom panelId names the panel", custom.id === "custom:x");
check("a title function receives the params", custom.title === "Custom x");
check("the custom type opens in its default zone", zoneOfPanel(custom) === "main");
check("a closable: false type opens in its default zone", zoneOfPanel(pinned) === "left");
check("the closable flag is kept on the entry", panelTypeEntry("pinned")?.closable === false);
check("the lazy chunk mounts each panel's content", [single, first, second, custom, pinned].every(probeContent));
check(
  "the dockview params reach the panel",
  probeContent(first)?.dataset.params === JSON.stringify({ instance: "a" }),
);
check(
  "reopening an open instance reveals it instead of duplicating",
  openInZone("probe", { instance: "a" }) === first && dock.panels.length === 5,
);

// --- Windows ids split at the first colon ---------------------------------------

const windowsId = panelIdFor("custom", { name: "C:\\dir\\f.txt" });
setZoneOverride(windowsId, "left");
check("a moved panel records its override", getService(ZONE_STATE).overrideFor(windowsId) === "left");
setZoneOverride(windowsId, "main");
check(
  "an id holding a drive colon resolves its type, so moving home clears the override",
  getService(ZONE_STATE).overrideFor(windowsId) === undefined,
);

// --- Save, then restore on a fresh dock ------------------------------------------

const envelope = JSON.parse(JSON.stringify(buildLayoutEnvelope(dock)));
const relaunched = createDock(window.document.createElement("div"));
initZones(relaunched);
check("the saved layout restores", restoreLayout(relaunched, envelope) === true);
relaunched.layout(1200, 800);
await flush();

const restoredIds = relaunched.panels.map((panel) => panel.id).sort();
check(
  "every panel comes back under its id",
  JSON.stringify(restoredIds) === JSON.stringify(["custom:x", "pinned", "probe", "probe:a", "probe:b"]),
);
check("the restored title-function title holds", relaunched.getPanel("custom:x")?.title === "Custom x");
check(
  "the restored instance keeps its params",
  probeContent(relaunched.getPanel("probe:b"))?.dataset.params === JSON.stringify({ instance: "b" }),
);
check(
  "restored panels return to their zones",
  zoneOfPanel(relaunched.getPanel("probe:a")) === "right" &&
    zoneOfPanel(relaunched.getPanel("custom:x")) === "main" &&
    zoneOfPanel(relaunched.getPanel("pinned")) === "left",
);

// --- A registered layout policy the core never names ----------------------------

// The default layout and its anchors are product policy: the composition
// root registers them, and the layout core applies whatever is
// registered. This policy has the product's shape over the synthetic
// types - a non-closable left anchor and a right anchor - and counts its
// seeds so a default layout is told apart from a restore.
let seeds = 0;
registerService(LAYOUT_POLICY, () => ({
  anchors: ["pinned", "probe"],
  seed: () => {
    seeds += 1;
    openInZone("pinned", {});
    openInZone("probe", {});
  },
}));

const seeded = createDock(window.document.createElement("div"));
initZones(seeded);
applyLayoutOrDefault(seeded, null);
await flush();
check("a null apply seeds the registered policy once", seeds === 1);
check(
  "the seeded anchors open in their zones",
  zoneOfPanel(seeded.getPanel("pinned")) === "left" && zoneOfPanel(seeded.getPanel("probe")) === "right",
);

// A layout saved after the right anchor closed away restores, and the
// anchor re-opens into the zone's surviving group.
seeded.removePanel(seeded.getPanel("probe"));
const lostAnchor = JSON.parse(JSON.stringify(buildLayoutEnvelope(seeded)));
const restored = createDock(window.document.createElement("div"));
initZones(restored);
applyLayoutOrDefault(restored, lostAnchor);
await flush();
check("a restore never seeds the policy", seeds === 1);
check(
  "a restored layout that lost an anchor gets it back in its zone",
  zoneOfPanel(restored.getPanel("probe")) === "right" && restored.panels.length === 2,
);

// The Secondary Side Bar toggle on a dock whose right zone was never
// built opens the policy's right-zone anchors, and only those.
const bare = createDock(window.document.createElement("div"));
initZones(bare);
resetZones();
openInZone("custom", { name: "doc" });
const contextKeys = getService(CONTEXT_KEY_SERVICE);
await Commands.execute("workbench.action.toggleAuxiliaryBar");
await flush();
check(
  "the side bar toggle opens the policy's right anchor when the right zone was never built",
  zoneOfPanel(bare.getPanel("probe")) === "right" && contextKeys.getValue("auxiliaryBarVisible") === true,
);
check("the side bar toggle opens no anchor outside the right zone", bare.getPanel("pinned") === undefined);
await Commands.execute("workbench.action.toggleAuxiliaryBar");
check(
  "the next toggle hides the right anchor's group",
  bare.getPanel("probe")?.group.api.isVisible === false && contextKeys.getValue("auxiliaryBarVisible") === false,
);

if (failures.length > 0) {
  console.error(`layout-open-registry: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("layout-open-registry: all assertions passed");
process.exit(0);
