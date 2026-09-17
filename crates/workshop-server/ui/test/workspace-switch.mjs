// Unit test for the workspace switch carrying UI state (plan step 13:
// src/ui/workspace-files/workspace-files.contribution.ts over the
// UI-state adapter, the dock, the tree state, and the closed-editor
// stack). Bundles the contribution with esbuild - the Tauri dialog and
// event modules aliased to the recording stubs in test/helpers - and
// drives Open Workspace from File..., Save Workspace As..., and Duplicate
// Workspace... against jsdom with a scripted fetch, the fake UI-state
// adapter (test/helpers/ui-storage.mjs), a fake dock bound through
// initZones, and live TreeStateService and ClosedEditors instances.
// Covers: after a successful Open the adapter's workspace bucket is
// reloaded, dock.fromJSON receives the opened file's layout, the tree's
// expanded set and the closed-editor stack are replaced wholesale, and
// no workspace write lands during or after the apply: the fake dock
// delivers its layout-change event on a microtask as Dockview does, the
// real debounced startLayoutPersistence is wired as main.ts wires it, and
// the check waits past the debounce so the saver's restore echo is caught
// rather than hidden by the apply's synchronous suppression; a real
// layout change after the Open still saves; a refused Open reloads and
// applies nothing; after a
// successful Save As exactly three workspace writes carry the live
// layout envelope, expanded set, and closed stack; a cancelled or
// refused Save As writes nothing; and Duplicate writes nothing. The fake
// dock hosts a real WorkshopTreePanel as its "tree" panel and a real
// WindowTitle reads the roots beside it, so the switch's roots traffic
// is the real thing (UM-001, OP-001): once /workspace/file/open has
// resolved the tree never renders the previous workspace's roots, and
// the panel and the title together fetch GET /workspace/tree exactly
// once for the switch.
// Run: node --test test/workspace-switch.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/ui/workspace-files/workspace-files.contribution.ts";
      export { register } from "./src/ui/workspace-files/index.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { registerService } from "./src/services/service-registry.ts";
      export { UI_STORAGE } from "./src/services/ui-storage.ts";
      export { TREE_STATE, TreeStateService } from "./src/services/tree-state-service.ts";
      export { CLOSED_EDITORS, ClosedEditors } from "./src/ui/editor/closed-editors.ts";
      export { initZones } from "./src/ui/layout/zones.ts";
      export { LAYOUT_SCHEMA_VERSION, startLayoutPersistence } from "./src/ui/layout/layout-persistence.ts";
      export { STATUS_BAR } from "./src/ui/status/status-bar.ts";
      export { WorkshopTreePanel } from "./src/ui/layout/workshop-panel.ts";
      export { WindowTitle } from "./src/ui/chrome/command-center.ts";
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
  alias: {
    "@tauri-apps/plugin-dialog": path.join(uiDir, "helpers", "tauri-dialog-stub.mjs"),
    "@tauri-apps/api/event": path.join(uiDir, "helpers", "tauri-event-stub.mjs"),
  },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.HTMLInputElement = window.HTMLInputElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;
globalThis.MutationObserver = window.MutationObserver;

const bundlePath = path.join(os.tmpdir(), "promptforge-workspace-switch-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  register,
  Commands,
  registerService,
  UI_STORAGE,
  TREE_STATE,
  TreeStateService,
  CLOSED_EDITORS,
  ClosedEditors,
  initZones,
  LAYOUT_SCHEMA_VERSION,
  startLayoutPersistence,
  STATUS_BAR,
  WorkshopTreePanel,
  WindowTitle,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets an async action chain (dialog -> fetch -> json -> reload -> emit) settle.
async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// Waits out the layout saver's 250 ms debounce, so a write it armed
// would have landed by the time the caller looks.
function settleLayoutSaver() {
  return new Promise((resolve) => setTimeout(resolve, 400));
}

// --- The fake dock -----------------------------------------------------------

// A dock good enough for the zone registry, the layout restore, and the
// layout snapshot, with Dockview's event timing: every layout change is
// delivered through one coalesced microtask (dockview-core's AsapEvent),
// never synchronously, and fromJSON rebuilds groups and panels from the
// serialized grid, queues that microtask through the adds, then fires
// onDidLayoutFromJSON synchronously at its end. A saver wired to
// onDidLayoutChange therefore reacts to a restore only after the apply's
// synchronous span has ended, which is what the real Open path faces.
// toJSON answers a distinctive live grid so a Save As snapshot is
// recognizable; fireLayoutChange stands in for a user's drag or close.
// The "tree" panel is a real WorkshopTreePanel, created and init'd on
// add and disposed on clear as Dockview re-creates panel content through
// fromJSON, so the switch drives the real roots load.
function makeFakeDock() {
  const layoutListeners = new Set();
  const fromJsonListeners = new Set();
  const panels = new Map();
  let layoutChangeQueued = false;
  let treePanel = null;
  let liveGrid = { root: { type: "leaf", data: { views: [], activeView: undefined, id: "live" }, size: 1 }, width: 1, height: 1, orientation: "HORIZONTAL" };
  const dock = {
    fromJSONCalls: [],
    clears: 0,
    groups: [],
    get panels() {
      return [...panels.values()];
    },
    onDidMovePanel: () => ({ dispose() {} }),
    onDidLayoutChange: (fn) => {
      layoutListeners.add(fn);
      return { dispose: () => layoutListeners.delete(fn) };
    },
    onDidLayoutFromJSON: (fn) => {
      fromJsonListeners.add(fn);
      return { dispose: () => fromJsonListeners.delete(fn) };
    },
    fireLayoutChange: () => {
      if (layoutChangeQueued) return;
      layoutChangeQueued = true;
      queueMicrotask(() => {
        layoutChangeQueued = false;
        for (const listener of layoutListeners) listener();
      });
    },
    getPanel: (id) => panels.get(id),
    getGroup: (id) => dock.groups.find((group) => group.id === id),
    addPanel: (options) => addPanel(options.id, `g-${options.id}`, options.params),
    clear: () => {
      dock.clears += 1;
      if (panels.size > 0 || dock.groups.length > 0) dock.fireLayoutChange();
      panels.clear();
      dock.groups.length = 0;
      treePanel?.dispose();
      treePanel?.element.remove();
      treePanel = null;
    },
    /** The live tree panel's element, for reading the rendered roots. */
    get treeElement() {
      return treePanel?.element ?? null;
    },
    fromJSON: (layout) => {
      dock.fromJSONCalls.push(layout);
      dock.clear();
      const walk = (node) => {
        if (node.type === "branch") {
          for (const child of node.data) walk(child);
          return;
        }
        for (const view of node.data.views) addPanel(view, node.data.id, {});
      };
      walk(layout.grid.root);
      liveGrid = layout.grid;
      for (const listener of fromJsonListeners) listener();
    },
    toJSON: () => ({ grid: liveGrid, panels: Object.fromEntries([...panels.keys()].map((id) => [id, { id }])), activeGroup: dock.groups[0]?.id }),
  };
  function addPanel(id, groupId, params) {
    let group = dock.getGroup(groupId);
    if (group === undefined) {
      group = { id: groupId, api: { setSize() {}, isVisible: true, setVisible() {} } };
      dock.groups.push(group);
    }
    const panel = { id, params, group, api: { setActive() {} } };
    panels.set(id, panel);
    if (id === "tree") {
      treePanel = new WorkshopTreePanel(null);
      treePanel.init();
      window.document.body.appendChild(treePanel.element);
    }
    dock.fireLayoutChange();
    return panel;
  }
  return dock;
}

// --- The scripted server -----------------------------------------------------

const fetches = [];
const answerQueue = [];
// The tree's traffic sits outside the ordered queue: the roots (GET
// /workspace/tree with no path) answer with whatever `rootsListing`
// holds, a directory answers empty. Each roots fetch is counted in
// `rootsFetches` so the switch's total can be asserted.
const dir = (name, p) => ({ name, path: p, kind: "directory", size: 0, modified_ms: 1, exists: true });
const LIVE_ROOTS = { path: null, entries: [dir("src", "C:\\work\\src")] };
const FILE_ROOTS = { path: null, entries: [dir("beta", "C:\\beta")] };
let rootsListing = LIVE_ROOTS;
let rootsFetches = 0;
// Set when the scripted server answers POST /workspace/file/open: from
// then on the tree must never paint the previous workspace's roots.
let openResolved = false;

// Every root row the tree paints after the open resolved, by path. The
// observer sees each render's list items as they land in the roots list
// (the record's target); the row is the item's direct child button. Read
// from the record, not the live tree, because a later render may already
// have cleared the item by the time the observer's microtask runs -
// which is exactly the flash this catches.
const rootsRendered = [];
function rootRowOf(item) {
  return [...item.children].find((child) => child.classList.contains("ws-workshop-tree__row")) ?? null;
}
const treeRenders = new MutationObserver((records) => {
  if (!openResolved) return;
  for (const record of records) {
    if (!(record.target instanceof window.Element) || !record.target.classList.contains("ws-workshop-tree__list")) continue;
    for (const node of record.addedNodes) {
      const row = node instanceof window.Element ? rootRowOf(node) : null;
      if (row !== null) rootsRendered.push(row.title);
    }
  }
});
treeRenders.observe(window.document.body, { childList: true, subtree: true });

globalThis.fetch = async (url, init) => {
  const parsed = new URL(url, "http://127.0.0.1:7912/");
  if (parsed.pathname === "/workspace/tree") {
    const p = parsed.searchParams.get("path");
    if (p === null) {
      rootsFetches += 1;
      return { ok: true, status: 200, json: async () => rootsListing };
    }
    return { ok: true, status: 200, json: async () => ({ path: p, entries: [] }) };
  }
  fetches.push({ url, method: init?.method ?? "GET", body: init?.body === undefined ? null : JSON.parse(init.body) });
  const answer = answerQueue.shift();
  if (answer === undefined) {
    throw new Error(`unexpected fetch in the workspace-switch test: ${url}`);
  }
  if (url === "/workspace/file/open" && answer.status < 400) {
    treeRenders.takeRecords();
    rootsRendered.length = 0;
    openResolved = true;
  }
  return { ok: answer.status < 400, status: answer.status, json: async () => answer.body };
};

/** The root paths the live tree panel shows right now. */
function rootsOnScreen() {
  return [...(dock.treeElement?.querySelectorAll(".ws-workshop-tree__list > li") ?? [])].map((item) => rootRowOf(item)?.title ?? "");
}

const statusMessages = [];
registerService(STATUS_BAR, () => ({
  showLocal: (label, severity) => statusMessages.push({ label, severity }),
}));

window.__TAURI_INTERNALS__ = {};
window.__TAURI_DIALOG__ = { calls: [], answer: null };
window.__TAURI_EVENTS__ = { emitted: [], fail: false };

const CURRENT = {
  path: "C:\\work\\Alpha.pfwork",
  name: "Alpha",
  grants: [{ path: "C:\\work\\src", exists: true }],
  window_state: null,
};

// --- The stores, bound the way main.ts binds them -------------------------------

// The live arrangement before any switch: what the previous workspace held.
const LIVE_EXPANDED = ["C:\\work\\src", "C:\\work\\src\\lib"];
const LIVE_CLOSED = ["C:\\work\\src\\old.md"];

const storage = createFakeUiStorage({
  workspace: { tree: { expanded: LIVE_EXPANDED }, closed_editors: { paths: LIVE_CLOSED } },
});
let reloads = 0;
// The opened file's bucket: what the real adapter's reloadWorkspace pulls.
let fileState = {};
storage.reloadWorkspace = async () => {
  reloads += 1;
  storage.replaceWorkspace(fileState);
};
registerService(UI_STORAGE, () => storage);

const tree = new TreeStateService(storage.get("workspace", "tree"), (value) => storage.set("workspace", "tree", value));
registerService(TREE_STATE, () => tree);
const closed = new ClosedEditors(storage.get("workspace", "closed_editors"), (value) =>
  storage.set("workspace", "closed_editors", value),
);
registerService(CLOSED_EDITORS, () => closed);

const dock = makeFakeDock();
initZones(dock);
dock.addPanel({ id: "tree", params: {} });
dock.addPanel({ id: "agent", params: {} });
// The window title reads the roots through the shared load, as the
// command center does; its default listRoots is the real one.
const title = new WindowTitle();
// Let the boot layout's change event pass before the saver subscribes, so
// the only layout writes the test sees are the ones the switch causes.
await flush();
check("the boot tree and title share one roots fetch", rootsFetches === 1);
check("the boot tree shows the live workspace's roots", rootsOnScreen().join(",") === "C:\\work\\src");
check("the boot title is the first live root", window.document.title === "src");
// The live layout saver, exactly as main.ts wires it: debounced, off the
// dock's microtask-delivered change events, writing into the workspace
// bucket. Nothing here is undebounced or synchronous, so the Open echo
// the real page would produce is the one this test can catch.
const layoutSaver = startLayoutPersistence(dock, (value) => storage.set("workspace", "layout", value));

register();

const workspaceSets = () => storage.sets.filter((entry) => entry.bucket === "workspace");

// --- Open: the file's arrangement replaces the live one -------------------------

const OPENED_PATH = "C:\\work\\Beta.pfwork";
const FILE_LAYOUT = {
  version: LAYOUT_SCHEMA_VERSION,
  zones: { left: "beta-left", right: "beta-right" },
  overrides: { "editor:C:\\beta\\notes.md": "right" },
  layout: {
    grid: {
      root: {
        type: "branch",
        data: [
          { type: "leaf", data: { views: ["tree"], activeView: "tree", id: "beta-left" }, size: 30 },
          { type: "leaf", data: { views: ["agent"], activeView: "agent", id: "beta-right" }, size: 70 },
        ],
        size: 100,
      },
      width: 100,
      height: 100,
      orientation: "HORIZONTAL",
    },
    panels: { tree: { id: "tree" }, agent: { id: "agent" } },
    activeGroup: "beta-right",
  },
};
const FILE_EXPANDED = ["C:\\beta", "C:\\beta\\docs"];
const FILE_CLOSED = ["C:\\beta\\docs\\readme.md", "C:\\beta\\notes.md"];

{
  fileState = { layout: FILE_LAYOUT, tree: { expanded: FILE_EXPANDED }, closed_editors: { paths: FILE_CLOSED } };
  const rootsFetchesBefore = rootsFetches;
  // The server switches its grants with the open: the roots it answers
  // from here on are the file's.
  rootsListing = FILE_ROOTS;
  window.__TAURI_DIALOG__.answer = OPENED_PATH;
  answerQueue.push({ status: 200, body: { ...CURRENT, path: OPENED_PATH, name: "Beta" } });
  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  check("a successful open posts the picked path", fetches.at(-1)?.url === "/workspace/file/open" && fetches.at(-1)?.body?.path === OPENED_PATH);
  check("the switch fetches the roots exactly once across the tree and the title", rootsFetches === rootsFetchesBefore + 1);
  check("once the open resolved the tree never renders the previous workspace's roots", !rootsRendered.includes("C:\\work\\src"));
  check("every root the tree renders after the open is the opened workspace's", rootsRendered.length > 0 && rootsRendered.every((p) => p === "C:\\beta"));
  check("the re-created tree shows one copy of the opened workspace's root", rootsOnScreen().join(",") === "C:\\beta");
  check("the title follows the opened workspace's first root", window.document.title === "beta");
  check("a successful open reloads the workspace bucket once", reloads === 1);
  check("dock.fromJSON receives the opened file's layout once", dock.fromJSONCalls.length === 1 && dock.fromJSONCalls[0] === FILE_LAYOUT.layout);
  check("the dock holds the file's groups after the apply", dock.groups.map((group) => group.id).join(",") === "beta-left,beta-right");
  check("the expanded set is replaced with the file's", isDeepStrictEqual([...tree.expandedPaths], FILE_EXPANDED));
  check("the closed stack is replaced with the file's, most recent first", isDeepStrictEqual(closed.snapshot().paths, FILE_CLOSED));
  check("no workspace write lands during the apply", workspaceSets().length === 0);
  check("a successful open fires the workspace-changed invalidation", window.__TAURI_EVENTS__.emitted.at(-1)?.payload?.path === OPENED_PATH);
  check("a successful open paints nothing on the status bar", statusMessages.length === 0);

  // The restore's layout-change event arrived on a microtask, after the
  // apply's suppression lifted, and the saver's debounce would have landed
  // the echo by now; the saver dropped it instead.
  await settleLayoutSaver();
  check("no workspace write lands after the apply, past the saver's debounce", workspaceSets().length === 0);
  check(
    "the layout saver's restore echo never reaches the adapter, suppressed or otherwise",
    storage.suppressed.filter((entry) => entry.key === "layout").length === 0,
  );

  // The saver is still live: a real layout change after the Open saves
  // the current arrangement once.
  dock.fireLayoutChange();
  await settleLayoutSaver();
  check(
    "a real layout change after the open still saves once",
    workspaceSets().length === 1 && workspaceSets()[0].key === "layout" && workspaceSets()[0].value.layout.grid === FILE_LAYOUT.layout.grid,
  );
}

// --- Open: a refusal reloads and applies nothing --------------------------------

{
  const reloadsBefore = reloads;
  const fromJsonBefore = dock.fromJSONCalls.length;
  const setsBefore = storage.sets.length;
  window.__TAURI_DIALOG__.answer = "C:\\elsewhere\\notes.txt";
  answerQueue.push({ status: 400, body: { error: { code: "refused", message: "not a PromptForge workspace" } } });
  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  check("a refused open paints the error", statusMessages.at(-1)?.severity === "error");
  check("a refused open reloads nothing", reloads === reloadsBefore);
  check("a refused open applies no layout", dock.fromJSONCalls.length === fromJsonBefore);
  check("a refused open leaves the expanded set alone", isDeepStrictEqual([...tree.expandedPaths], FILE_EXPANDED));
  check("a refused open leaves the closed stack alone", isDeepStrictEqual(closed.snapshot().paths, FILE_CLOSED));
  await settleLayoutSaver();
  check("a refused open writes nothing", storage.sets.length === setsBefore);
}

// --- Save As: the new file carries the live arrangement ---------------------------

const SAVED_PATH = "C:\\work\\Gamma.pfwork";
{
  // Interactive changes since the open, so the live values differ from
  // the file's: a folder opened (no write yet: the tree debounces), and
  // one more editor closed (one immediate write, counted out below).
  tree.replaceExpanded([...FILE_EXPANDED, "C:\\beta\\src"]);
  closed.push({ kind: "file", path: "C:\\beta\\src\\main.rs" });
  closed.push({ kind: "untitled", text: "draft" });
  const setsBefore = storage.sets.length;
  const liveExpanded = [...tree.expandedPaths];
  const liveClosed = closed.snapshot();

  window.__TAURI_DIALOG__.answer = SAVED_PATH;
  answerQueue.push(
    { status: 200, body: { ...CURRENT, path: OPENED_PATH, name: "Beta" } },
    { status: 200, body: { ...CURRENT, path: SAVED_PATH, name: "Gamma" } },
  );
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  check("a picked name is posted to /workspace/file/save_as", fetches.at(-1)?.url === "/workspace/file/save_as" && fetches.at(-1)?.body?.path === SAVED_PATH);
  const sets = storage.sets.slice(setsBefore);
  check("a successful save-as makes exactly three workspace writes", sets.length === 3 && sets.every((entry) => entry.bucket === "workspace"));
  check("the three writes cover layout, tree, and closed_editors once each", [...new Set(sets.map((entry) => entry.key))].sort().join(",") === "closed_editors,layout,tree");
  const layout = sets.find((entry) => entry.key === "layout")?.value;
  check(
    "the layout write is the live envelope: current schema, the zone map, the dock's own snapshot",
    layout?.version === LAYOUT_SCHEMA_VERSION &&
      isDeepStrictEqual(layout?.zones, FILE_LAYOUT.zones) &&
      isDeepStrictEqual(layout?.overrides, FILE_LAYOUT.overrides) &&
      layout?.layout?.grid === FILE_LAYOUT.layout.grid &&
      Object.keys(layout?.layout?.panels ?? {}).sort().join(",") === "agent,tree",
  );
  check("the tree write carries the live expanded set", isDeepStrictEqual(sets.find((entry) => entry.key === "tree")?.value, { expanded: liveExpanded }));
  check(
    "the closed_editors write carries the live file stack, untitled buffers excluded",
    isDeepStrictEqual(sets.find((entry) => entry.key === "closed_editors")?.value, liveClosed) &&
      liveClosed.paths[0] === "C:\\beta\\src\\main.rs",
  );
  check("a successful save-as reloads nothing", reloads === 1);
  check("a successful save-as emits the new path", window.__TAURI_EVENTS__.emitted.at(-1)?.payload?.path === SAVED_PATH);
}

// --- Save As: a cancel or a refusal writes nothing ------------------------------------

{
  const setsBefore = storage.sets.length;
  window.__TAURI_DIALOG__.answer = null;
  answerQueue.push({ status: 200, body: { ...CURRENT, path: SAVED_PATH, name: "Gamma" } });
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  check("a cancelled save-as writes nothing", storage.sets.length === setsBefore);

  window.__TAURI_DIALOG__.answer = "C:\\work\\Alpha.pfwork";
  answerQueue.push(
    { status: 200, body: { ...CURRENT, path: SAVED_PATH, name: "Gamma" } },
    { status: 409, body: { error: { code: "conflict", message: "workspace file already exists" } } },
  );
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  check("a refused save-as paints the error", statusMessages.at(-1)?.label.includes("already exists") === true);
  check("a refused save-as writes nothing", storage.sets.length === setsBefore);
}

// --- Duplicate: unchanged, writes nothing and applies nothing ---------------------------

{
  const setsBefore = storage.sets.length;
  const fromJsonBefore = dock.fromJSONCalls.length;
  const reloadsBefore = reloads;
  window.__TAURI_DIALOG__.answer = "C:\\work\\Gamma copy";
  answerQueue.push(
    { status: 200, body: { ...CURRENT, path: SAVED_PATH, name: "Gamma" } },
    { status: 200, body: { ...CURRENT, path: "C:\\work\\Gamma copy.pfwork", name: "Gamma copy" } },
  );
  await Commands.execute("workbench.action.duplicateWorkspace");
  await flush();
  check("a successful duplicate posts to /workspace/file/duplicate", fetches.at(-1)?.url === "/workspace/file/duplicate");
  check("a successful duplicate writes no workspace state", storage.sets.length === setsBefore);
  check("a successful duplicate applies no layout", dock.fromJSONCalls.length === fromJsonBefore && reloads === reloadsBefore);
}

check("every queued server answer was consumed", answerQueue.length === 0);

layoutSaver.dispose();
treeRenders.disconnect();
title.dispose();
dock.clear();
tree.dispose();

if (failures.length > 0) {
  console.error(`workspace-switch: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workspace-switch: all assertions passed");
