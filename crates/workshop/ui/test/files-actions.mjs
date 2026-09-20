// Unit test for the File menu's pickers and file actions (plan step 16:
// src/parts/workspace/files.contribution.ts, file-actions.ts, and the
// add-folder flow lifted out of the Workshop tree panel). Bundles the
// contribution with esbuild - "@tauri-apps/plugin-dialog" aliased to the
// scripted stub in test/helpers - and drives the commands through the
// shared registries against jsdom. Covers: the catalog wiring (titles,
// menu groups, preconditions, chord labels, palette rows); Open File
// granting and opening the picked path, with cancel a no-op; Open Folder
// and Add Folder to Workspace sharing one flow - the native picker on
// desktop, the typed-path dialog in a plain browser; Save As granting
// the target's parent before writing and retargeting the panel, with
// cancel a no-op; Save All writing every dirty editor sequentially and
// skipping clean ones; and Revert File prompting on unsaved changes
// before reloading from disk. Plan step 17 adds: the Open Recent menu's
// dynamic root and recent-file rows merged with the static More... and
// Clear Recently Opened... rows; the "" quick-access provider over the
// recent-files store and the tree's fetched listings, deduped and
// substring-filtered, whose accept opens an editor through vscode.open;
// vscode.open and vscode.openFolder narrowing their path argument;
// vscode.openFolder fetching, caching, and expanding an uncached root
// before focusing the tree; clearRecentFiles emptying the store; and
// openRecent showing quick open at the "" list.
// Run: node --test test/files-actions.mjs
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
      import "./src/parts/workspace/files.contribution.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus } from "./src/services/menu-registry.ts";
      export { KeybindingsRegistry } from "./src/services/keybinding-registry.ts";
      export { QuickAccessRegistry } from "./src/services/quick-access-registry.ts";
      export { RECENT_FILES_STORE, RecentFilesStore } from "./src/services/recent-files-store.ts";
      export { TREE_STATE, TreeStateService } from "./src/services/tree-state-service.ts";
      export { registerService } from "./src/services/service-registry.ts";
      export { DOCK } from "./src/services/panel-registry.ts";
      export { QUICK_INPUT_SERVICE } from "./src/parts/quickinput/quick-input.ts";
      export { EditorPanel } from "./src/parts/editor/editor-panel.ts";
      export { initZones } from "./src/parts/layout/zones.ts";
      export { STATUS_BAR } from "./src/parts/status/status-bar.ts";
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
  },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;

// CodeMirror measures text through Range, which jsdom does not layout;
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
if (!window.HTMLElement.prototype.getBoundingClientRect) {
  window.HTMLElement.prototype.getBoundingClientRect = zeroRect;
}
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

for (const key of [
  "document",
  "navigator",
  "HTMLElement",
  "HTMLInputElement",
  "HTMLButtonElement",
  "Node",
  "Element",
  "Range",
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
// Node ships its own Event and CustomEvent globals, so the copy loop skips
// them - but events the bundle dispatches into the jsdom document must be
// jsdom-realm instances: jsdom's dispatchEvent rejects Node's Event.
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;

// The contribution registers at module scope; a malformed descriptor
// reports through console.error, so spy on it across the bundle import.
const consoleErrors = [];
const realConsoleError = console.error;
console.error = (...args) => {
  consoleErrors.push(args.join(" "));
};

const bundlePath = path.join(os.tmpdir(), "promptforge-files-actions-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  Commands,
  Menus,
  KeybindingsRegistry,
  QuickAccessRegistry,
  RECENT_FILES_STORE,
  RecentFilesStore,
  TREE_STATE,
  TreeStateService,
  registerService,
  DOCK,
  QUICK_INPUT_SERVICE,
  EditorPanel,
  initZones,
  STATUS_BAR,
} = await import(pathToFileURL(bundlePath).href);
console.error = realConsoleError;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets an async action chain (fetch -> json -> dialog) run to completion.
async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- Shared fakes ------------------------------------------------------------

const statusMessages = [];
registerService(STATUS_BAR, () => ({
  showLocal: (label, severity) => statusMessages.push({ label, severity }),
}));

// One ordered event log across grants and writes, so Save As can be
// pinned to grant-the-parent-before-write.
const events = [];
const grants = [];
// Scripted GET /workspace/tree answers, keyed by directory path ("" is
// the synthetic granted-roots listing). vscode.openFolder reads these.
const treeListings = new Map();
let treeFetches = 0;
globalThis.fetch = async (url, init) => {
  if (url === "/workspace/grant") {
    const granted = JSON.parse(init.body).path;
    grants.push(granted);
    events.push(`grant:${granted}`);
    return { ok: true, status: 200, json: async () => ({ granted }) };
  }
  if (url === "/workspace/tree" || url.startsWith("/workspace/tree?")) {
    treeFetches += 1;
    const dir = new URL(url, "http://127.0.0.1").searchParams.get("path") ?? "";
    const listing = treeListings.get(dir) ?? { path: dir === "" ? null : dir, entries: [] };
    return { ok: true, status: 200, json: async () => listing };
  }
  throw new Error(`unexpected fetch in the files-actions test: ${url}`);
};

let workspaceChanges = 0;
window.addEventListener("promptforge:workspace-changed", () => {
  workspaceChanges += 1;
});

// A recording EditorSurface stand-in: dirty is divergence from the last
// open/markSaved baseline, exactly the real surface's contract.
function fakeSurface(text) {
  let current = text;
  let baseline = text;
  const listeners = [];
  return {
    element: window.document.createElement("div"),
    open(doc) {
      current = doc.text;
      baseline = doc.text;
    },
    text: () => current,
    markSaved(t) {
      baseline = t;
    },
    isDirty: () => current !== baseline,
    setReadOnly() {},
    onDirtyChange(listener) {
      listeners.push(listener);
      return () => {};
    },
    editorView: () => null,
    focus() {},
    dispose() {},
    // Test-only edit seam: types text and notifies dirty listeners.
    setText(t) {
      current = t;
      for (const listener of listeners) listener(current !== baseline);
    },
  };
}

const writeCalls = [];
let writesInFlight = 0;
let maxWritesInFlight = 0;
const writeFileStub = async (path, text, token) => {
  writesInFlight += 1;
  maxWritesInFlight = Math.max(maxWritesInFlight, writesInFlight);
  writeCalls.push({ path, text, token });
  events.push(`write:${path}`);
  await new Promise((resolve) => setTimeout(resolve, 10));
  writesInFlight -= 1;
  return { path, size: text.length, token: "tok-next", text };
};

// Builds an EditorPanel over a fake surface, mounted and loaded.
function makeEditorPanel({ path: filePath, text, readFile }) {
  const surface = fakeSurface(text);
  const titles = [];
  const panel = new EditorPanel({
    createSurface: () => surface,
    readFile:
      readFile ?? (async (p) => ({ path: p, size: text.length, token: "tok-1", text })),
    writeFile: writeFileStub,
  });
  panel.init({
    params: { path: filePath },
    api: { setTitle: (title) => titles.push(title), close() {} },
  });
  window.document.body.appendChild(panel.element);
  return { panel, surface, titles };
}

// --- Catalog wiring ----------------------------------------------------------

check("the contribution registers without a malformed descriptor", consoleErrors.length === 0);

{
  const fileRows = Menus.getMenuItems("menubar/file");
  const paletteRows = Menus.getMenuItems("commandPalette");
  const expected = [
    ["workbench.action.files.openFile", "Open File...", "2_open", "!isWeb", "Ctrl+O"],
    ["workbench.action.files.openFolder", "Open Folder...", "2_open", undefined, "Ctrl+M Ctrl+O"],
    ["workbench.action.addRootFolder", "Add Folder to Workspace...", "3_workspace", undefined, undefined],
    ["workbench.action.files.saveAs", "Save As...", "4_save", "!isWeb && activeEditor", "Ctrl+Shift+S"],
    ["workbench.action.files.saveAll", "Save All", "4_save", undefined, "Ctrl+M S"],
    ["workbench.action.files.revert", "Revert File", "6_close", "activeEditor", undefined],
  ];
  for (const [id, title, group, precondition, label] of expected) {
    const command = Commands.lookup(id);
    check(`${id} is registered with its catalog title`, command?.title === title);
    check(`${id} carries its catalog precondition`, command?.precondition === precondition);
    const row = fileRows.find((r) => r.command === id);
    check(`${id} sits in the File menu's ${group} group`, row?.group === group);
    check(
      `${id} reaches the command palette`,
      paletteRows.some((r) => r.command === id),
    );
    const keybinding = KeybindingsRegistry.lookupKeybinding(id);
    check(
      `${id} binds ${label ?? "no chord"}`,
      label === undefined ? keybinding === undefined : keybinding?.getLabel() === label,
    );
  }
}

// --- Open File: pick, grant, open; cancel is a no-op -------------------------

window.__TAURI_INTERNALS__ = {};
const addedPanels = [];
initZones({
  panels: [],
  groups: [],
  getPanel: (id) => addedPanels.find((p) => p.id === id),
  // The stub tracks no live groups; a recorded zone group reads as closed
  // away, and openInZone rebuilds the zone - dockview's own self-healing.
  getGroup: () => undefined,
  addPanel: (opts) => {
    const panel = {
      id: opts.id,
      params: opts.params,
      group: { id: "g-main" },
      api: { setActive() {} },
      // focusWorkshopTree unwraps view.content before its instanceof check.
      view: { content: {} },
    };
    addedPanels.push(panel);
    return panel;
  },
  onDidMovePanel: () => ({ dispose() {} }),
  onDidRemovePanel: () => ({ dispose() {} }),
  onWillMutateLayout: () => ({ dispose() {} }),
  onDidMutateLayout: () => ({ dispose() {} }),
  onDidLayoutChange: () => ({ dispose() {} }),
});

{
  window.__TAURI_DIALOG__ = { calls: [], answer: null };
  await Commands.execute("workbench.action.files.openFile");
  await flush();
  check("a cancelled Open File grants nothing", grants.length === 0);
  check("a cancelled Open File opens no editor", addedPanels.length === 0);

  window.__TAURI_DIALOG__.answer = "C:\\picked\\notes.txt";
  await Commands.execute("workbench.action.files.openFile");
  await flush();
  check(
    "Open File opens the native file picker",
    window.__TAURI_DIALOG__.calls.some((c) => c.kind === "open" && c.directory !== true),
  );
  check("Open File grants the picked path", grants.includes("C:\\picked\\notes.txt"));
  check("Open File announces one workspace change", workspaceChanges === 1);
  check(
    "Open File opens an editor on the picked path",
    addedPanels.some((p) => p.id === "editor:C:\\picked\\notes.txt"),
  );
}

// --- Open Folder / Add Folder to Workspace: one shared flow ------------------

{
  const grantsBefore = grants.length;
  window.__TAURI_DIALOG__.answer = null;
  await Commands.execute("workbench.action.files.openFolder");
  await flush();
  check(
    "Open Folder opens the native directory picker",
    window.__TAURI_DIALOG__.calls.at(-1)?.kind === "open" &&
      window.__TAURI_DIALOG__.calls.at(-1)?.directory === true,
  );
  check("a cancelled Open Folder grants nothing", grants.length === grantsBefore);

  window.__TAURI_DIALOG__.answer = "C:\\picked-dir";
  await Commands.execute("workbench.action.files.openFolder");
  await flush();
  check("Open Folder grants the picked folder", grants.at(-1) === "C:\\picked-dir");
  check(
    "a granted folder confirms on the status bar as info",
    statusMessages.some(
      (e) => e.severity === "info" && e.label.includes("Added") && e.label.includes("C:\\picked-dir"),
    ),
  );

  window.__TAURI_DIALOG__.answer = "C:\\added-dir";
  await Commands.execute("workbench.action.addRootFolder");
  await flush();
  check("Add Folder to Workspace runs the same flow", grants.at(-1) === "C:\\added-dir");
}

// --- Browser mode: the typed-path dialog fallback ----------------------------

{
  delete window.__TAURI_INTERNALS__;
  await Commands.execute("workbench.action.files.openFolder");
  await flush();
  const overlay = window.document.querySelector(".ws-workspace-add-overlay");
  check("browser Open Folder opens the typed-path dialog", overlay !== null);
  const input = overlay?.querySelector("input#workspace-add-path");
  const addButton = [...(overlay?.querySelectorAll("button") ?? [])].find(
    (b) => b.textContent === "Add",
  );
  check("the dialog gates Add on a typed path", addButton?.disabled === true);
  input.value = "C:\\typed-dir";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  addButton.click();
  await flush();
  check("the typed path is granted", grants.at(-1) === "C:\\typed-dir");
  check(
    "the dialog dismisses after Add",
    window.document.querySelector(".ws-workspace-add-overlay") === null,
  );
  window.__TAURI_INTERNALS__ = {};
}

// --- Save As: grant the parent, write, retarget; cancel is a no-op -----------

{
  const { panel, surface, titles } = makeEditorPanel({ path: "C:\\project\\a.txt", text: "original" });
  await flush();
  surface.setText("changed");
  const fakePanel = { view: { content: panel } };
  const unregister = registerService(DOCK, () => ({ activePanel: fakePanel, panels: [fakePanel] }));

  window.__TAURI_DIALOG__ = { calls: [], answer: null };
  const writesBefore = writeCalls.length;
  await Commands.execute("workbench.action.files.saveAs");
  await flush();
  check("a cancelled Save As writes nothing", writeCalls.length === writesBefore);
  check("a cancelled Save As keeps the panel's path", panel.filePath() === "C:\\project\\a.txt");

  window.__TAURI_DIALOG__.answer = "C:\\elsewhere\\b.txt";
  await Commands.execute("workbench.action.files.saveAs");
  await flush();
  check(
    "Save As opens the native save dialog seeded with the current path",
    window.__TAURI_DIALOG__.calls.some(
      (c) => c.kind === "save" && c.defaultPath === "C:\\project\\a.txt",
    ),
  );
  check("Save As grants the target's parent directory", grants.includes("C:\\elsewhere"));
  check(
    "Save As grants the parent before writing",
    events.indexOf("grant:C:\\elsewhere") !== -1 &&
      events.indexOf("grant:C:\\elsewhere") < events.indexOf("write:C:\\elsewhere\\b.txt"),
  );
  check(
    "Save As writes the live text to the new path with no conflict token",
    writeCalls.some((w) => w.path === "C:\\elsewhere\\b.txt" && w.text === "changed" && w.token === null),
  );
  check("Save As retargets the panel onto the new path", panel.filePath() === "C:\\elsewhere\\b.txt");
  check("Save As leaves the panel clean", panel.isDirty() === false);
  check("Save As retitles the tab", titles.includes("b.txt"));
  unregister.dispose();
  panel.dispose();
}

// --- Save All: every dirty editor, sequentially ------------------------------

{
  const a = makeEditorPanel({ path: "C:\\project\\a.txt", text: "aaa" });
  const b = makeEditorPanel({ path: "C:\\project\\b.txt", text: "bbb" });
  const c = makeEditorPanel({ path: "C:\\project\\c.txt", text: "ccc" });
  await flush();
  a.surface.setText("aaa edited");
  b.surface.setText("bbb edited");
  // c stays clean.
  const panels = [a, b, c].map(({ panel }) => ({ view: { content: panel } }));
  const unregister = registerService(DOCK, () => ({ activePanel: panels[0], panels }));
  const writesBefore = writeCalls.length;
  maxWritesInFlight = 0;
  await Commands.execute("workbench.action.files.saveAll");
  await flush();
  const written = writeCalls.slice(writesBefore).map((w) => w.path);
  check(
    "Save All writes every dirty editor",
    written.includes("C:\\project\\a.txt") && written.includes("C:\\project\\b.txt"),
  );
  check("Save All skips clean editors", !written.includes("C:\\project\\c.txt"));
  check("Save All saves sequentially, never concurrently", maxWritesInFlight === 1);
  check(
    "Save All leaves every saved editor clean",
    !a.panel.isDirty() && !b.panel.isDirty() && c.panel.isDirty() === false,
  );
  unregister.dispose();
  for (const { panel } of [a, b, c]) panel.dispose();
}

// --- Revert File: the dirty prompt, then a reload from disk ------------------

{
  let reads = 0;
  const readFile = async (p) => {
    reads += 1;
    return { path: p, size: 7, token: "tok-1", text: "on disk" };
  };
  const { panel, surface } = makeEditorPanel({ path: "C:\\project\\r.txt", text: "on disk", readFile });
  await flush();
  check("the panel loaded its file once", reads === 1);
  surface.setText("edited");
  const fakePanel = { view: { content: panel } };
  const unregister = registerService(DOCK, () => ({ activePanel: fakePanel, panels: [fakePanel] }));

  await Commands.execute("workbench.action.files.revert");
  await flush();
  const overlay = panel.element.querySelector(".ws-editor-revert-overlay");
  check("reverting a dirty editor prompts first", overlay !== null);
  check("the prompt holds the reload", reads === 1);
  const cancel = [...(overlay?.querySelectorAll("button") ?? [])].find((b) => b.textContent === "Cancel");
  cancel.click();
  await flush();
  check("cancelling the revert keeps the edits", surface.text() === "edited" && panel.isDirty());
  check("cancelling the revert reloads nothing", reads === 1);

  await Commands.execute("workbench.action.files.revert");
  await flush();
  const confirmOverlay = panel.element.querySelector(".ws-editor-revert-overlay");
  const revertButton = [...(confirmOverlay?.querySelectorAll("button") ?? [])].find(
    (b) => b.textContent === "Revert",
  );
  revertButton.click();
  await flush();
  check("confirming the revert reloads the file", reads === 2);
  check("the reverted editor shows the on-disk text", surface.text() === "on disk");
  check("the reverted editor is clean", panel.isDirty() === false);

  await Commands.execute("workbench.action.files.revert");
  await flush();
  check(
    "reverting a clean editor skips the prompt",
    panel.element.querySelector(".ws-editor-revert-overlay") === null,
  );
  check("a clean revert still reloads from disk", reads === 3);
  unregister.dispose();
  panel.dispose();
}

// --- Revert on an untitled buffer is a no-op ---------------------------------

{
  const panel = new EditorPanel({ createSurface: () => fakeSurface(""), writeFile: writeFileStub });
  panel.init({ params: { untitled: 1 }, api: { setTitle() {}, close() {} } });
  window.document.body.appendChild(panel.element);
  await flush();
  const fakePanel = { view: { content: panel } };
  const unregister = registerService(DOCK, () => ({ activePanel: fakePanel, panels: [fakePanel] }));
  await Commands.execute("workbench.action.files.revert");
  await flush();
  check(
    "reverting an untitled buffer opens no prompt",
    panel.element.querySelector(".ws-editor-revert-overlay") === null,
  );
  unregister.dispose();
  panel.dispose();
}

// --- Step 17: Open Recent catalog wiring -------------------------------------

// Real DOM-free service instances bound over the self-registered defaults;
// the contribution's provider and run bodies resolve them at call time.
const treeState = new TreeStateService();
const recentStore = new RecentFilesStore(null);
registerService(TREE_STATE, () => treeState);
registerService(RECENT_FILES_STORE, () => recentStore);

{
  const recentRows = Menus.getMenuItems("menubar/file/recent");
  check(
    "More... sits in Open Recent's y_more group",
    recentRows.some((r) => r.command === "workbench.action.openRecent" && r.group === "y_more"),
  );
  check(
    "Clear Recently Opened... sits in Open Recent's z_clear group",
    recentRows.some((r) => r.command === "workbench.action.clearRecentFiles" && r.group === "z_clear"),
  );
  check(
    "More... binds Ctrl+R",
    KeybindingsRegistry.lookupKeybinding("workbench.action.openRecent")?.getLabel() === "Ctrl+R",
  );
  const paletteRows = Menus.getMenuItems("commandPalette");
  check(
    "Open Recent's More... reaches the palette",
    paletteRows.some((r) => r.command === "workbench.action.openRecent"),
  );
  check(
    "Clear Recently Opened reaches the palette",
    paletteRows.some((r) => r.command === "workbench.action.clearRecentFiles"),
  );
  check(
    "vscode.open is registered but stays out of the palette",
    Commands.lookup("vscode.open") !== undefined && !paletteRows.some((r) => r.command === "vscode.open"),
  );
}

// --- Step 17: provider rows and the "" quick-access provider -----------------

{
  // Seed: one granted root with a fetched listing, two recent files.
  treeState.cacheListing("", {
    path: null,
    entries: [{ name: "project", path: "C:\\project", kind: "directory", size: 0, modifiedMs: 1, exists: true }],
  });
  treeState.cacheListing("C:\\project", {
    path: "C:\\project",
    entries: [
      { name: "a.txt", path: "C:\\project\\a.txt", kind: "file", size: 3, modifiedMs: 2, exists: true },
      { name: "b.txt", path: "C:\\project\\b.txt", kind: "file", size: 3, modifiedMs: 3, exists: true },
    ],
  });
  // Most recent first: a.txt was "opened" after notes.txt.
  recentStore.add("C:\\picked\\notes.txt");
  recentStore.add("C:\\project\\a.txt");

  // Action rows carry no title; the widget falls back to the command's.
  const titles = Menus.getMenuItems("menubar/file/recent").map(
    (r) => r.title ?? Commands.lookup(r.command)?.title,
  );
  check(
    "Open Recent lists roots, then recent files, then the static rows",
    titles.join(",") === "project,a.txt,notes.txt,More...,Clear Recently Opened...",
  );

  const descriptor = QuickAccessRegistry.getQuickAccessProvider("b.txt");
  check("an unprefixed query routes to the '' provider", descriptor?.prefix === "");
  const provider = descriptor.factory();
  const items = provider.getItems("");
  check(
    "the '' provider lists recent files first, then tree files, deduped",
    items.map((i) => i.description).join(",") === "C:\\project\\a.txt,C:\\picked\\notes.txt,C:\\project\\b.txt",
  );
  check(
    "the '' provider labels rows with the file's base name",
    items.map((i) => i.label).join(",") === "a.txt,notes.txt,b.txt",
  );
  check(
    "the '' provider filters by case-insensitive substring",
    provider.getItems("NOTES").map((i) => i.label).join(",") === "notes.txt",
  );

  const panelsBefore = addedPanels.length;
  items.find((i) => i.description === "C:\\project\\b.txt").accept();
  await flush();
  check(
    "accepting a '' row opens an editor on the file",
    addedPanels.length === panelsBefore + 1 && addedPanels.some((p) => p.id === "editor:C:\\project\\b.txt"),
  );

  await Commands.execute("vscode.open");
  await Commands.execute("vscode.open", 42);
  await flush();
  check(
    "vscode.open narrows its argument: a missing or non-string path opens nothing",
    addedPanels.length === panelsBefore + 1,
  );

  await Commands.execute("workbench.action.clearRecentFiles");
  await flush();
  check("Clear Recently Opened empties the store", recentStore.list.length === 0);
  check(
    "the cleared store drops the recent-file rows",
    !Menus.getMenuItems("menubar/file/recent").some((r) => r.title === "notes.txt"),
  );
}

// --- Step 17: More... opens quick open; vscode.openFolder focuses a root ----

{
  const shown = [];
  registerService(QUICK_INPUT_SERVICE, () => ({ quickAccess: { show: (value) => shown.push(value) } }));
  await Commands.execute("workbench.action.openRecent");
  await flush();
  check("More... opens quick open at the '' file list", shown.length === 1 && shown[0] === "");

  // The wire shape: modified_ms, snake_case, as the server answers.
  treeListings.set("C:\\picked-dir", {
    path: "C:\\picked-dir",
    entries: [{ name: "notes.txt", path: "C:\\picked-dir\\notes.txt", kind: "file", size: 5, modified_ms: 4, exists: true }],
  });
  const changesBefore = workspaceChanges;
  await Commands.execute("vscode.openFolder", "C:\\picked-dir");
  await flush();
  check(
    "vscode.openFolder fetches and caches an uncached root's listing",
    treeState.listing("C:\\picked-dir") !== undefined,
  );
  check("vscode.openFolder expands the root in the tree state", treeState.isExpanded("C:\\picked-dir"));
  check(
    "vscode.openFolder announces the workspace change so an open tree re-renders",
    workspaceChanges === changesBefore + 1,
  );
  check("vscode.openFolder focuses the tree panel", addedPanels.some((p) => p.id === "tree"));

  const fetchesBefore = treeFetches;
  await Commands.execute("vscode.openFolder", "C:\\picked-dir");
  await flush();
  check(
    "vscode.openFolder reuses the cached listing on a second focus",
    treeFetches === fetchesBefore,
  );

  await Commands.execute("vscode.openFolder", 42);
  await flush();
  check("vscode.openFolder narrows its argument", treeState.listing("42") === undefined);
}

if (failures.length > 0) {
  console.error(`files-actions: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("files-actions: all assertions passed");
