// Integration test for the workshop zone registry and file tree
// (src/workshop/zones.ts, panel-types.ts, workshop-panel.ts). Bundles the
// modules with esbuild, mounts a real Dockview dock in jsdom against the
// real index.html, and drives the public API. Covers: Agent and Workshop
// panels mount through the registry; multiple Agent panels coexist as
// tabs with stable per-agent ids; the tree requests directory paths
// from /workspace/tree and never file contents; openInZone places panels
// by affinity and honors per-panel overrides; a zone group is rebuilt
// after its last panel closes; tree expansion survives a panel reopen.
// Run: node test/workshop-zones.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

// One bundle, one module graph: zones, panel-types, and the tree panel
// share their module state, and dockview comes along for a real dock.
const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createDockview, themeDark } from "dockview";
      export {
        initZones,
        openAgentPanel,
        openInZone,
        panelIdFor,
        setZoneOverride,
        zoneOfPanel,
      } from "./src/workshop/zones.ts";
      export { createPanelComponent, isPanelType } from "./src/workshop/panel-types.ts";
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
});

const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
const { window } = dom;

// The same layout stubs the smoke test installs: jsdom has no layout.
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

// Opening a file mounts a real CodeMirror editor (step 14); jsdom has no
// layout, so Range and element measurement get zero-rect shims. The test
// never asserts editor geometry.
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

// A scripted workspace API. The roots listing carries one directory;
// expanding it returns a folder and two files (folders first, as the
// server orders them). /workspace/file serves a small text per path so
// the step-14 editor panels can load; any other route rejects, so an
// unexpected request fails the test loudly.
const calls = [];
const ROOT = "C:\\project";
const LISTINGS = new Map([
  [
    null,
    { path: null, entries: [{ name: "project", path: ROOT, kind: "directory", size: 0, modified_ms: 100 }] },
  ],
  [
    ROOT,
    {
      path: ROOT,
      entries: [
        { name: "src", path: `${ROOT}\\src`, kind: "directory", size: 0, modified_ms: 100 },
        { name: "a.txt", path: `${ROOT}\\a.txt`, kind: "file", size: 3, modified_ms: 100 },
        { name: "b.txt", path: `${ROOT}\\b.txt`, kind: "file", size: 3, modified_ms: 100 },
      ],
    },
  ],
  // "broken" has no listing: expanding it exercises the fetch-failure path.
  [
    `${ROOT}\\src`,
    {
      path: `${ROOT}\\src`,
      entries: [
        { name: "broken", path: `${ROOT}\\src\\broken`, kind: "directory", size: 0, modified_ms: 100 },
      ],
    },
  ],
]);
globalThis.fetch = async (url) => {
  calls.push(url);
  if (typeof url === "string" && url.startsWith("/workspace/file")) {
    const pathParam = new URL(url, "http://127.0.0.1:7910").searchParams.get("path");
    if (pathParam !== null && pathParam.startsWith(ROOT)) {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          path: pathParam,
          size: 7,
          modified_ms: 100,
          text: `text of ${pathParam}`,
        }),
      };
    }
  }
  if (typeof url === "string" && url.startsWith("/workspace/tree")) {
    const query = url.includes("?") ? new URL(url, "http://127.0.0.1:7910").searchParams : null;
    const pathParam = query ? query.get("path") : null;
    const key = pathParam === null || pathParam === "" ? null : pathParam;
    if (LISTINGS.has(key)) {
      return { ok: true, status: 200, json: async () => LISTINGS.get(key) };
    }
  }
  throw new Error(`unexpected fetch in the workshop-zones test: ${url}`);
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

// The bundle includes all of dockview, so it is far too large for a data
// URL (a failure would print the whole megabyte-long URL). Import from a
// temp file instead, which also keeps stack traces readable.
const bundlePath = path.join(os.tmpdir(), "promptforge-workshop-zones-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  initZones,
  openAgentPanel,
  openInZone,
  panelIdFor,
  setZoneOverride,
  zoneOfPanel,
  createPanelComponent,
  isPanelType,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets the async fetch/render chains run to completion.
async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

function rowByText(text) {
  return [...window.document.querySelectorAll(".workshop-tree__row")].find(
    (row) => row.textContent === text,
  );
}

const treeCalls = () => calls.filter((url) => url.startsWith("/workspace/tree"));
const fileCalls = () => calls.filter((url) => url.startsWith("/workspace/file"));

// The dock, wired exactly as main.ts wires it.
const dock = createDockview(window.document.getElementById("dock"), {
  createComponent: createPanelComponent,
  theme: themeDark,
  singleTabMode: "fullwidth",
  disableFloatingGroups: true,
  hideBorders: true,
  locked: true,
  noPanelsOverlay: "emptyGroup",
});
initZones(dock);

// --- Boot: the first Agent and the Workshop mount through the registry -----

const chatPanel = openAgentPanel();
const treePanel = openInZone("tree", {});
await flush();

check("boot opens exactly the agent and tree panels", dock.panels.length === 2);
check("the agent panel mounts its .mur-app surface", !!window.document.querySelector("#dock .mur-app"));
check("the Workshop tree panel mounts", !!window.document.querySelector("#dock .workshop-tree"));
check("the agent opens in the right zone", zoneOfPanel(chatPanel) === "right");
check("the tree opens in the left zone", zoneOfPanel(treePanel) === "left");
check("boot renders two zone groups", dock.groups.length === 2);
check(
  "reopening a singleton panel activates it instead of duplicating",
  openInZone("tree", {}) === treePanel && dock.panels.length === 2,
);

// --- Agent tabs: every New Agent is a fresh panel in the right bank --------

const agentB = openAgentPanel();
check("a second agent opens as a distinct panel", agentB !== chatPanel && dock.panels.length === 3);
check(
  "agent panels carry stable per-agent ids",
  chatPanel.id.startsWith("chat:") && agentB.id.startsWith("chat:") && chatPanel.id !== agentB.id,
);
check(
  "agent tabs default to the Agent title",
  chatPanel.title === "Agent" && agentB.title === "Agent",
);
check(
  "coexisting agents share the right-zone group as tabs",
  agentB.group.id === chatPanel.group.id && dock.groups.length === 2,
);
check(
  "the second agent mounts its own .mur-app surface",
  agentB.view.content.element.querySelector(".mur-app") !== null &&
    agentB.view.content.element.querySelector(".mur-app") !==
      chatPanel.view.content.element.querySelector(".mur-app"),
);
check(
  "panelIdFor honors a provided agent id",
  panelIdFor("chat", { agentId: "fixed" }) === "chat:fixed",
);
check(
  "panelIdFor generates unique agent ids without one",
  panelIdFor("chat", {}) !== panelIdFor("chat", {}),
);
// A restored layout reopens an agent by its persisted id: same panel, no copy.
const fixedAgent = openInZone("chat", { agentId: "fixed" });
check(
  "reopening an agent by id activates the same panel",
  openInZone("chat", { agentId: "fixed" }) === fixedAgent && dock.panels.length === 4,
);
check("an agent opened by id lands in the right zone", zoneOfPanel(fixedAgent) === "right");
dock.removePanel(fixedAgent);
check("closing one agent leaves the others open", dock.panels.length === 3);

// --- The tree requests paths, never file contents ---------------------------

check("the tree fetched the granted roots on mount", treeCalls().includes("/workspace/tree"));
check("no file contents were requested while browsing", fileCalls().length === 0);
const projectRow = rowByText("project");
check("the granted root renders as a directory row", !!projectRow);
check("the root row starts collapsed", projectRow?.getAttribute("aria-expanded") === "false");

// Expand the root: one request carrying the directory path.
projectRow.click();
await flush();
const expandUrl = `/workspace/tree?path=${encodeURIComponent(ROOT)}`;
check("expanding a directory requests its path", calls.includes(expandUrl));
check("expansion marks the row expanded", projectRow.getAttribute("aria-expanded") === "true");
const childNames = [
  ...window.document.querySelectorAll(".workshop-tree__children .workshop-tree__name"),
].map((span) => span.textContent);
check(
  "children render folders before files in server order",
  childNames.join(",") === "src,a.txt,b.txt",
);
check("still no file contents requested after expansion", fileCalls().length === 0);

// --- File activation opens an editor in the main zone -----------------------

rowByText("a.txt").click();
const editorA = dock.getPanel(panelIdFor("editor", { path: `${ROOT}\\a.txt` }));
check("activating a file opens an editor panel", !!editorA);
check("the editor opens in the main zone", editorA && zoneOfPanel(editorA) === "main");
check("a third zone group appears for main", dock.groups.length === 3);
await flush();
check(
  "opening a file requests its contents through the workspace API",
  fileCalls().includes(`/workspace/file?path=${encodeURIComponent(`${ROOT}\\a.txt`)}`),
);
check(
  "the editor panel mounts its CodeMirror surface",
  !!editorA && !!editorA.api && !!window.document.querySelector(".editor-panel .cm-editor"),
);

// A second editor lands within the same main group (affinity, not a new zone).
const editorB = openInZone("editor", { path: `${ROOT}\\b.txt` });
check(
  "a second editor joins the existing main group",
  editorB.group.id === editorA.group.id && dock.groups.length === 3,
);
check(
  "the main group is neither the chat nor the tree group",
  editorB.group.id !== chatPanel.group.id && editorB.group.id !== treePanel.group.id,
);

// --- Overrides: a moved panel reopens in its chosen zone --------------------

const bId = panelIdFor("editor", { path: `${ROOT}\\b.txt` });
setZoneOverride(bId, "right");
dock.removePanel(editorB);
const editorB2 = openInZone("editor", { path: `${ROOT}\\b.txt` });
check("an overridden panel reopens in the override zone", zoneOfPanel(editorB2) === "right");
check(
  "the override lands the panel within the right zone's group",
  editorB2.group.id === chatPanel.group.id,
);
// Moving back to the affinity zone deletes the override.
setZoneOverride(bId, "main");
dock.removePanel(editorB2);
const editorB3 = openInZone("editor", { path: `${ROOT}\\b.txt` });
check(
  "resetting to the affinity zone deletes the override",
  zoneOfPanel(editorB3) === "main" && editorB3.group.id === editorA.group.id,
);

// --- A zone group rebuilds after its last panel closes ----------------------

const mainGroupId = editorA.group.id;
dock.removePanel(editorA);
dock.removePanel(editorB3);
check("closing every main panel removes its group", dock.getGroup(mainGroupId) === undefined);
check("only the chat and tree groups remain", dock.groups.length === 2);
const editorC = openInZone("editor", { path: `${ROOT}\\c.txt` });
check("the next main-zone open rebuilds the group", dock.groups.length === 3);
check(
  "the rebuilt group is a fresh group in the main zone",
  editorC.group.id !== mainGroupId && zoneOfPanel(editorC) === "main",
);

// --- Tree reopen: expansion state survives for the session ------------------

const callsBeforeReopen = treeCalls().length;
dock.removePanel(treePanel);
check("closing the tree removes the left zone group", dock.groups.length === 2);
const treePanel2 = openInZone("tree", {});
await flush();
check("the tree reopens in the left zone", zoneOfPanel(treePanel2) === "left");
check("the left zone group rebuilds on reopen", dock.groups.length === 3);
check(
  "a reopened tree renders from the session cache without refetching",
  treeCalls().length === callsBeforeReopen,
);
const reopenedProjectRow = rowByText("project");
check(
  "the reopened tree keeps the root expanded",
  reopenedProjectRow?.getAttribute("aria-expanded") === "true",
);
const reopenedChildList = reopenedProjectRow?.closest("li")?.querySelector(".workshop-tree__children");
check(
  "the reopened tree shows the cached children",
  !!reopenedChildList && !reopenedChildList.hidden && !!rowByText("a.txt"),
);

// --- A failed expansion surfaces a visible error row ------------------------

const srcRow = rowByText("src");
check("the nested directory renders after reopen", !!srcRow);
srcRow.click();
await flush();
const brokenRow = rowByText("broken");
check("a directory whose listing will fail renders as a row", !!brokenRow);
brokenRow.click();
await flush();
const brokenChildren = brokenRow?.closest("li")?.querySelector(".workshop-tree__children");
const errorRow = brokenChildren?.querySelector(".workshop-tree__error");
check("a failed expansion renders an error row", !!errorRow);
check(
  "the error row is visible rather than hidden with the collapsed list",
  !!brokenChildren && !brokenChildren.hidden,
);
check("the error row is announced as an alert", errorRow?.getAttribute("role") === "alert");
check(
  "a failed expansion leaves the row collapsed",
  brokenRow?.getAttribute("aria-expanded") === "false",
);

// --- Registry edge: unknown component names render a placeholder ------------

const unknown = createPanelComponent({ id: "x", name: "nope" });
check(
  "an unknown component name renders a labelled placeholder",
  unknown.element.textContent === "Unknown panel: nope",
);
check("isPanelType narrows registered names", isPanelType("editor") && !isPanelType("nope"));

if (failures.length > 0) {
  console.error(`workshop-zones: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-zones: all assertions passed");
process.exit(0);
