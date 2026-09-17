// Integration test for zone stability (src/ui/layout/zones.ts size memory
// and resurrection, the placeholder panel type, and the restore guard in
// layout-persistence.ts). Bundles the modules with esbuild, mounts real
// Dockview docks in jsdom, and drives the public API. Covers: closing the
// last editor leaves the side zones pixel-identical with the main group
// alive holding its placeholder at the recorded size; opening an editor
// reuses that group and drops the placeholder; closing the last agent
// panel preserves the right zone; an explicit group close lets the zone
// die; a mid-session restore spawns no placeholders; and a quit-and-
// relaunch restores all three zones at their prior sizes with the
// placeholders included.
// Run: node --test test/zone-stability.mjs
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
      export { createDockview, themeDark } from "dockview";
      export {
        initZones,
        openInZone,
        panelIdFor,
        resetZones,
        zoneOfPanel,
      } from "./src/ui/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/ui/layout/panel-types.ts";
      export { restoreLayout, buildLayoutEnvelope } from "./src/ui/layout/layout-persistence.ts";
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
window.IntersectionObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
};
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

// CodeMirror measurement shims; the test never asserts editor geometry.
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

// A scripted workspace API: one granted root with one file; file reads
// serve a small text. Any other route fails the test loudly.
const ROOT = "C:\\project";
const FILE_A = `${ROOT}\\a.txt`;
globalThis.fetch = async (url) => {
  const target = typeof url === "string" ? url : url.url;
  if (target.startsWith("/workspace/file")) {
    const pathParam = new URL(target, "http://127.0.0.1:7910").searchParams.get("path");
    return {
      ok: true,
      status: 200,
      json: async () => ({ path: pathParam, size: 7, token: "t100", text: `text of ${pathParam}` }),
    };
  }
  if (target.startsWith("/workspace/tree")) {
    return {
      ok: true,
      status: 200,
      json: async () => ({
        path: null,
        entries: [{ name: "project", path: ROOT, kind: "directory", size: 0, modified_ms: 100, exists: true }],
      }),
    };
  }
  throw new Error(`unexpected fetch in the zone-stability test: ${target}`);
};

for (const key of [
  "document",
  "navigator",
  "location",
  "localStorage",
  "Window",
  "HTMLElement",
  "HTMLTemplateElement",
  "Node",
  "Element",
  "Event",
  "CustomEvent",
  "MutationObserver",
  "Option",
  "DOMParser",
  "ResizeObserver",
  "IntersectionObserver",
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

// The agent panel composes an AgentSocket on init; a scripted stand-in
// that never opens keeps the panel inert.
globalThis.WebSocket = class {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  readyState = 0;
  send() {}
  close() {}
};

const bundlePath = path.join(os.tmpdir(), "zone-stability-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  initZones,
  openInZone,
  panelIdFor,
  resetZones,
  zoneOfPanel,
  createPanelComponent,
  createPanelTabComponent,
  restoreLayout,
  buildLayoutEnvelope,
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

// --- Closing the last editor: side zones hold, main resurrects --------------

const dock = createDock(window.document.getElementById("dock"));
initZones(dock);
resetZones();

const treePanel = openInZone("tree", {});
const agentPanel = openInZone("agent", {});
const editorA = openInZone("editor", { path: FILE_A });
await flush();

// Explicit sizes stand in for the user's arrangement; jsdom has no layout.
dock.layout(1200, 800);
const leftGroup = treePanel.group;
const rightGroup = agentPanel.group;
leftGroup.api.setSize({ width: 280 });
rightGroup.api.setSize({ width: 320 });
editorA.group.api.setSize({ width: 600 });
await flush();
check(
  "the arrangement took the scripted sizes",
  leftGroup.api.width === 280 && rightGroup.api.width === 320 && editorA.group.api.width === 600,
);

dock.removePanel(editorA);
await flush();

const mainPlaceholder = dock.getPanel(panelIdFor("placeholder", { zone: "main" }));
check("closing the last editor keeps a main placeholder alive", !!mainPlaceholder);
check("the placeholder lives in the main zone", !!mainPlaceholder && zoneOfPanel(mainPlaceholder) === "main");
check("the dock keeps all three zone groups", dock.groups.length === 3);
check(
  "the resurrected main group takes the recorded size",
  mainPlaceholder?.group.api.width === 600,
);
check(
  "the side zones are pixel-identical",
  leftGroup.api.width === 280 && rightGroup.api.width === 320,
);
check(
  "the placeholder tab cannot be closed from the tab strip",
  mainPlaceholder?.view.tab.element.querySelector(".dv-default-tab-action") === null,
);
await flush();
check(
  "the main placeholder renders its inert content",
  mainPlaceholder?.view.content.element.textContent.includes("Open a file to begin") === true,
);

// --- Opening an editor reuses the group and drops the placeholder -----------

const placeholderGroup = mainPlaceholder.group;
const editorB = openInZone("editor", { path: `${ROOT}\\b.txt` });
await flush();
check(
  "opening an editor reuses the placeholder's group",
  editorB.group.id === placeholderGroup.id,
);
check(
  "the placeholder is dropped when a real panel arrives",
  dock.getPanel(panelIdFor("placeholder", { zone: "main" })) === undefined,
);
check("the reused group keeps its size", editorB.group.api.width === 600);

// --- Closing the last agent panel preserves the right zone -------------------

dock.removePanel(editorB);
await flush();
dock.removePanel(agentPanel);
await flush();
const rightPlaceholder = dock.getPanel(panelIdFor("placeholder", { zone: "right" }));
check("closing the last agent panel keeps a right placeholder alive", !!rightPlaceholder);
check(
  "the right zone is preserved at its size",
  !!rightPlaceholder && zoneOfPanel(rightPlaceholder) === "right" && rightPlaceholder.group.api.width === 320,
);

// --- Quit and relaunch: zones restore at prior sizes, placeholders included --

// The quit snapshot: no editors open, the main and right zones holding
// their placeholders at the recorded sizes.
const envelope = JSON.parse(JSON.stringify(buildLayoutEnvelope(dock)));

// --- Explicit group closes ---------------------------------------------------

// Dockview's removeGroup closes each panel through the same per-panel
// path a tab close takes, so explicitly closing a group that holds a
// real panel resurrects the zone once - the plan's accepted fallback.
const editorC = openInZone("editor", { path: `${ROOT}\\c.txt` });
await flush();
const explicitGroup = editorC.group;
explicitGroup.api.close();
await flush();
check(
  "an explicit close of a real panel's group resurrects the zone once",
  !!dock.getPanel(panelIdFor("placeholder", { zone: "main" })),
);
// Closing the placeholder-only group explicitly is the workbench's way
// to retire a zone: no real panel closed, so the zone stays closed.
const inertGroup = dock.getPanel(panelIdFor("placeholder", { zone: "main" }))?.group;
inertGroup?.api.close();
await flush();
check(
  "an explicit close of the placeholder-only group removes it",
  inertGroup !== undefined && dock.getGroup(inertGroup.id) === undefined,
);
check(
  "an explicit close of the placeholder-only group spawns no placeholder",
  dock.getPanel(panelIdFor("placeholder", { zone: "main" })) === undefined &&
    !dock.panels.some((panel) => panel.id.startsWith("placeholder:main")),
);

// The relaunch: a fresh dock restores the snapshot. (initZones rebinds
// the module's dock, so the first dock is driven no further.)
const dockRelaunched = createDock(window.document.createElement("div"));
initZones(dockRelaunched);
check("the saved layout restores on relaunch", restoreLayout(dockRelaunched, envelope) === true);
// jsdom never sizes the dock element; a real relaunch lays the dock out
// to the window, which is what applies the serialized view sizes.
dockRelaunched.layout(1200, 800);
await flush();
check(
  "the relaunch restores every zone's placeholder through its factory",
  !!dockRelaunched.getPanel(panelIdFor("placeholder", { zone: "main" })) &&
    !!dockRelaunched.getPanel(panelIdFor("placeholder", { zone: "right" })) &&
    !!dockRelaunched.getPanel("tree"),
);
check("the relaunch restores all three zone groups", dockRelaunched.groups.length === 3);
check(
  "the relaunch restores the zones at their prior sizes",
  dockRelaunched.getPanel("tree")?.group.api.width === 280 &&
    dockRelaunched.getPanel(panelIdFor("placeholder", { zone: "main" }))?.group.api.width === 600 &&
    dockRelaunched.getPanel(panelIdFor("placeholder", { zone: "right" }))?.group.api.width === 320,
);

// --- A mid-session restore spawns no placeholders ------------------------------

// The Open Workspace path applies the opened file's envelope onto a live
// dock; fromJSON tears the live groups down first, and those removals
// must never resurrect.
const dockLive = createDock(window.document.createElement("div"));
initZones(dockLive);
resetZones();
openInZone("tree", {});
openInZone("agent", {});
openInZone("editor", { path: FILE_A });
await flush();
const opened = JSON.parse(JSON.stringify(buildLayoutEnvelope(dockLive)));
check(
  "the live layout carries no placeholders before the switch",
  !Object.keys(opened.layout.panels).some((id) => id.startsWith("placeholder:")),
);
check("the mid-session restore succeeds", restoreLayout(dockLive, opened) === true);
await flush();
check(
  "a layout restore spawns no placeholders",
  !dockLive.panels.some((panel) => panel.id.startsWith("placeholder:")),
);

if (failures.length > 0) {
  console.error(`zone-stability: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("zone-stability: all assertions passed");
process.exit(0);
