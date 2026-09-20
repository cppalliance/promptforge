// Integration test for zone stability (src/parts/layout/zones.ts size memory
// and empty-group rebuild, and the restore guard in layout-persistence.ts).
// Bundles the modules with esbuild, mounts real Dockview docks in jsdom,
// and drives the public API. Covers: closing the last editor leaves the
// main zone alive as an empty group at its recorded width with the side
// zones pixel-identical; reopening an editor lands in that same group;
// closing the last agent panel keeps the right zone; dragging a zone's
// last panel out leaves the zone alive and empty; relocating a zone's
// group keeps exactly one group for the zone; the layout envelope is v4
// and carries no placeholder panel; a relaunch restores the empty zones
// at their recorded widths with no panels; a mid-session restore and a
// reset create no extra groups; an unrelated mutation never creates a
// zone that was never opened; and nothing in the document reads
// "Placeholder".
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
      } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/parts/layout/panel-types.ts";
      export { restoreLayout, buildLayoutEnvelope } from "./src/parts/layout/layout-persistence.ts";
      export { getService } from "./src/services/service-registry.ts";
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
  getService,
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

/** The zone's live group on `dock` per the zone map, or undefined. */
function zoneGroup(dock, zone) {
  const id = getService(ZONE_STATE).groupFor(zone);
  return id === undefined ? undefined : dock.getGroup(id);
}

/** Every grid leaf (group record) in a serialized dockview layout. */
function gridLeaves(node, out = []) {
  if (node.type === "leaf") {
    out.push(node.data);
  } else if (node.type === "branch") {
    for (const child of node.data ?? []) gridLeaves(child, out);
  }
  return out;
}

/** Every grid leaf's view ids in a serialized dockview layout. */
function gridViewIds(node) {
  return gridLeaves(node).flatMap((leaf) => leaf?.views ?? []);
}

// --- Closing the last editor: side zones hold, main stays as an empty group -

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

const mainGroup = zoneGroup(dock, "main");
check("closing the last editor leaves the main zone with a live group", mainGroup !== undefined);
check("the rebuilt main group holds no panels", mainGroup?.panels.length === 0);
check("the dock keeps all three zone groups", dock.groups.length === 3);
check("the rebuilt main group takes the recorded width", mainGroup?.api.width === 600);
check(
  "the side zones are pixel-identical",
  leftGroup.api.width === 280 && rightGroup.api.width === 320,
);
check("the dock holds only the tree and agent panels", dock.panels.length === 2);

// --- Reopening an editor lands in the same empty group ----------------------

const editorB = openInZone("editor", { path: `${ROOT}\\b.txt` });
await flush();
check("the reopened editor lands in the rebuilt main group", editorB.group.id === mainGroup?.id);
check("the reopened editor's group keeps its width", editorB.group.api.width === 600);
check("the reopened editor is the main zone's only panel", editorB.group.panels.length === 1);
check("the reopened editor reports the main zone", zoneOfPanel(editorB) === "main");

// --- Closing the last agent panel keeps the right zone ----------------------

dock.removePanel(agentPanel);
await flush();
const rightRebuilt = zoneGroup(dock, "right");
check("closing the last agent panel leaves the right zone with a live group", rightRebuilt !== undefined);
check("the rebuilt right group holds no panels", rightRebuilt?.panels.length === 0);
check("the rebuilt right group takes the recorded width", rightRebuilt?.api.width === 320);
check("the dock still has three groups after the agent close", dock.groups.length === 3);

// --- Dragging a zone's last panel out leaves the zone alive and empty -------

// A panel drag onto another group is moveGroupOrPanel; the source group
// empties and dockview removes it inside the same mutation.
const mainBefore = editorB.group.id;
editorB.api.moveTo({ group: rightRebuilt });
await flush();
check("the dragged editor lands in the right zone's group", editorB.group.id === rightRebuilt?.id);
check("the drag-out reports the right zone for the editor", zoneOfPanel(editorB) === "right");
const mainAfterDrag = zoneGroup(dock, "main");
check("dragging the last panel out leaves the main zone alive", mainAfterDrag !== undefined);
check("the main zone after the drag-out is a fresh empty group", mainAfterDrag?.panels.length === 0 && mainAfterDrag?.id !== mainBefore);
check("the drag-out keeps three groups", dock.groups.length === 3);
// Drag it back so the main zone holds the editor again.
editorB.api.moveTo({ group: mainAfterDrag });
await flush();
check("dragging the editor back fills the empty main group", editorB.group.id === mainAfterDrag?.id);
check("the right zone is empty again after the drag back", rightRebuilt?.panels.length === 0);
check("the drag back keeps three groups", dock.groups.length === 3);

// --- Relocating a zone's group keeps exactly one group for the zone ---------

const mainGroupNow = editorB.group;
mainGroupNow.api.moveTo({ group: leftGroup, position: "left" });
await flush();
check("relocating the main group keeps its id in the zone map", zoneGroup(dock, "main")?.id === mainGroupNow.id);
check("relocating the main group creates no extra group", dock.groups.length === 3);
check("the relocated main group still holds the editor", mainGroupNow.panels.length === 1 && editorB.group.id === mainGroupNow.id);
mainGroupNow.api.moveTo({ group: leftGroup, position: "right" });
await flush();
check("relocating the main group back keeps three groups", dock.groups.length === 3);

// --- The envelope: v4, no placeholder panels or views -------------------------

dock.removePanel(editorB);
await flush();
check("the main zone survives the editor's close as an empty group", zoneGroup(dock, "main")?.panels.length === 0);
const envelope = JSON.parse(JSON.stringify(buildLayoutEnvelope(dock)));
check("the envelope reports schema version 4", envelope.version === 4);
check(
  "the envelope's panels carry no placeholder",
  !Object.keys(envelope.layout.panels).some((id) => id.startsWith("placeholder:")),
);
check(
  "the envelope's grid views carry no placeholder",
  !gridViewIds(envelope.layout.grid.root).some((id) => id.startsWith("placeholder:")),
);
check("the envelope's only panel is the tree", Object.keys(envelope.layout.panels).join(",") === "tree");
check(
  "the envelope serializes three grid leaves, two of them empty",
  gridLeaves(envelope.layout.grid.root).length === 3 &&
    gridLeaves(envelope.layout.grid.root).filter((leaf) => leaf.views.length === 0).length === 2,
);

// --- Relaunch: both empty zones restore at recorded widths with no panels ----

// initZones rebinds the module's dock, so the first dock is driven no
// further.
const dockRelaunched = createDock(window.document.createElement("div"));
initZones(dockRelaunched);
check("the saved layout restores on relaunch", restoreLayout(dockRelaunched, envelope) === true);
// jsdom never sizes the dock element; a real relaunch lays the dock out
// to the window, which is what applies the serialized view sizes.
dockRelaunched.layout(1200, 800);
await flush();
check("the relaunch restores all three zone groups", dockRelaunched.groups.length === 3);
check("the relaunch restores only the tree panel", dockRelaunched.panels.length === 1 && !!dockRelaunched.getPanel("tree"));
const relaunchedMain = zoneGroup(dockRelaunched, "main");
const relaunchedRight = zoneGroup(dockRelaunched, "right");
check("the relaunch restores the main zone as an empty group", relaunchedMain?.panels.length === 0);
check("the relaunch restores the right zone as an empty group", relaunchedRight?.panels.length === 0);
check(
  "the relaunch restores the zones at their prior widths",
  dockRelaunched.getPanel("tree")?.group.api.width === 280 &&
    relaunchedMain?.api.width === 600 &&
    relaunchedRight?.api.width === 320,
);
const relaunchedEditor = openInZone("editor", { path: FILE_A });
check("an editor opened after relaunch fills the restored empty main group", relaunchedEditor.group.id === relaunchedMain?.id);
check("filling the restored main group adds no group", dockRelaunched.groups.length === 3);

// --- A mid-session restore and a reset create no extra groups -----------------

// The Open Workspace path applies the opened file's envelope onto a live
// dock; fromJSON tears the live groups down first, and those removals
// must never rebuild anything.
const dockLive = createDock(window.document.createElement("div"));
initZones(dockLive);
resetZones();
openInZone("tree", {});
openInZone("agent", {});
openInZone("editor", { path: FILE_A });
await flush();
const opened = JSON.parse(JSON.stringify(buildLayoutEnvelope(dockLive)));
check("the mid-session restore succeeds", restoreLayout(dockLive, opened) === true);
await flush();
check("a mid-session restore creates no extra groups", dockLive.groups.length === 3);
check("a mid-session restore keeps every panel in a zone group", dockLive.groups.every((group) => group.panels.length === 1));
resetZones();
await flush();
check("a zone reset creates no groups", dockLive.groups.length === 3);

// --- A never-opened zone is not created by an unrelated mutation -------------

const dockTwo = createDock(window.document.createElement("div"));
initZones(dockTwo);
resetZones();
openInZone("tree", {});
openInZone("agent", {});
await flush();
const secondAgent = openInZone("agent", { instance: "second" });
await flush();
dockTwo.removePanel(secondAgent);
await flush();
check("an unrelated mutation leaves the never-opened main zone unmapped", zoneGroup(dockTwo, "main") === undefined);
check("an unrelated mutation creates no group for the never-opened zone", dockTwo.groups.length === 2);
check("the unrelated mutation keeps the right zone's group", zoneGroup(dockTwo, "right")?.panels.length === 1);

// --- No placeholder ever renders -------------------------------------------------

check(
  "no element in the document reads Placeholder",
  ![...window.document.body.querySelectorAll("*")].some((element) => element.textContent === "Placeholder"),
);
check("the placeholder panel type is gone from panelIdFor", panelIdFor("placeholder", { zone: "main" }) === "placeholder");

if (failures.length > 0) {
  console.error(`zone-stability: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("zone-stability: all assertions passed");
process.exit(0);
