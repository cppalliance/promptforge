// Integration test for layout boot, persistence, and shortcuts
// (src/parts/layout/layout-persistence.ts, src/parts/layout/layout-boot.ts,
// the keybinding dispatcher resolving the contribution surface's chords,
// the zone-state serialization in zones.ts, and EditorPanel.requestClose).
// Bundles the modules with esbuild, mounts real Dockview docks in jsdom
// against the real index.html, and drives the public API with a fake
// UI-state adapter (test/helpers/ui-storage.mjs) standing in for the
// workspace bucket. Covers: the layout survives a reload (build the
// envelope -> restore it), including the tree's close-button-free tab;
// the envelope omits the lock state; a burst of layout changes coalesces
// into one debounced write; a mid-session restore (the Open path) writes
// nothing while the next real change still saves; a throwing writer is
// logged, never escapes;
// stale schema versions (1 and 2) are rejected; a null, non-object,
// version-mismatched, or unloadable envelope falls back to defaults;
// applyLayoutOrDefault builds the default zones from null (clearing a live
// dock's panels first, the Open path) and restores a valid envelope
// through fromJSON; re-ensuring the tree after a restore
// never duplicates it; each shortcut dispatches its command; the status
// bar never enters the serialized layout.
// Run: node test/workshop-layout.mjs
import { readFile, writeFile } from "node:fs/promises";
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
      export { createDockview, themeDark } from "dockview";
      export {
        initZones,
        openInZone,
        panelIdFor,
        resetZones,
        zoneOfPanel,
      } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/parts/layout/panel-types.ts";
      export {
        restoreLayout,
        buildLayoutEnvelope,
        startLayoutPersistence,
        LAYOUT_SCHEMA_VERSION,
      } from "./src/parts/layout/layout-persistence.ts";
      export { applyLayoutOrDefault } from "./src/parts/layout/layout-boot.ts";
      export { KeybindingDispatcher } from "./src/parts/layout/keybinding-dispatcher.ts";
      export { CONTEXT_KEY_SERVICE } from "./src/services/context-key-service.ts";
      export { getService } from "./src/services/service-registry.ts";
      import "./src/parts/editor/editor.contribution.ts";
      import "./src/parts/layout/layout.contribution.ts";
      export { EditorPanel } from "./src/parts/editor/editor-panel.ts";
      export { StatusBar } from "./src/parts/status/status-bar.ts";
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

const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
const { window } = dom;

// The same layout stubs the other dock tests install: jsdom has no layout.
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

// A scripted workspace API: one granted root with two files; file reads
// serve a small text per path; writes record their bodies and bump the
// conflict token. Any other route fails the test loudly.
const ROOT = "C:\\project";
const FILE_A = `${ROOT}\\a.txt`;
const FILE_B = `${ROOT}\\b.txt`;
const FILE_C = `${ROOT}\\c.txt`;
const puts = [];
let tokenSeq = 100;
globalThis.fetch = async (url, options) => {
  const target = typeof url === "string" ? url : url.url;
  if (target.startsWith("/workspace/file") && !options) {
    const pathParam = new URL(target, "http://127.0.0.1:7910").searchParams.get("path");
    if (pathParam !== null && pathParam.startsWith(ROOT)) {
      return {
        ok: true,
        status: 200,
        json: async () => ({
          path: pathParam,
          size: 7,
          token: `t${tokenSeq}`,
          text: `text of ${pathParam}`,
        }),
      };
    }
  }
  if (target === "/workspace/file" && options?.method === "PUT") {
    const body = JSON.parse(options.body);
    puts.push(body);
    tokenSeq += 100;
    return {
      ok: true,
      status: 200,
      json: async () => ({
        path: body.path,
        size: body.text.length,
        token: `t${tokenSeq}`,
        text: body.text,
      }),
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
  throw new Error(`unexpected fetch in the workshop-layout test: ${target}`);
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
// that never opens keeps the panel inert - this test drives layout, not
// the agent wire.
globalThis.WebSocket = class {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  readyState = 0;
  send() {}
  close() {}
};

const bundlePath = path.join(os.tmpdir(), "workshop-layout-test.mjs");
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
  startLayoutPersistence,
  LAYOUT_SCHEMA_VERSION,
  applyLayoutOrDefault,
  KeybindingDispatcher,
  CONTEXT_KEY_SERVICE,
  getService,
  EditorPanel,
  StatusBar,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// A structural copy with every undefined-valued key removed, and nothing
// else changed: plain objects and arrays recurse, every other value
// (including a Map, a Date, or NaN) passes through untouched so a deep
// comparison against the JSON form still flags it.
function withoutUndefined(value) {
  if (Array.isArray(value)) {
    return value.map(withoutUndefined);
  }
  if (value !== null && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, item]) => item !== undefined)
        .map(([key, item]) => [key, withoutUndefined(item)]),
    );
  }
  return value;
}

// The workspace-bucket key the composition root binds the layout to.
const KEY = "layout";

// Builds a dock wired exactly as main.ts wires it, on a fresh element.
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

const editorAId = panelIdFor("editor", { path: FILE_A });
const editorBId = panelIdFor("editor", { path: FILE_B });
const editorCId = panelIdFor("editor", { path: FILE_C });

// --- Boot: the default layout is three-zone, always unlocked --------------

const dockEl = window.document.getElementById("dock");
const dock = createDockview(dockEl, {
  createComponent: createPanelComponent,
  createTabComponent: createPanelTabComponent,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
initZones(dock);

check("a null envelope has nothing to restore", restoreLayout(dock, null) === false);

const treePanel = openInZone("tree", {});
treePanel.group.api.setSize({ width: 280 });
const agentPanel = openInZone("agent", {});
await flush();

check("the default layout mounts tree and agent session", dock.panels.length === 2);
check("main stays empty until a document opens", dock.groups.length === 2);
check("the tree opens left, agent right", zoneOfPanel(treePanel) === "left" && zoneOfPanel(agentPanel) === "right");

// App placement: an editor opens into the main zone.
const editorA = openInZone("editor", { path: FILE_A });
await flush();
check("app placement lands the editor in main", zoneOfPanel(editorA) === "main");

// --- Persistence: the envelope is versioned and records placement ---------

const built = buildLayoutEnvelope(dock);
// The adapter PUTs JSON.stringify(value) and the boot preload hands back
// response.json(): a wire round trip must reproduce every defined value.
// Dockview's toJSON emits its optional panel fields (params, renderer,
// pinned, size limits, tabComponent) as explicit undefined, which JSON
// drops and fromJSON reads identically, so those keys are ignored; deep
// equality over the rest catches a non-JSON type (a Map, a Date, NaN) in
// the builder that a stringify-to-stringify comparison never could.
const envelope = JSON.parse(JSON.stringify(built));
check("the envelope survives the JSON wire round trip",
  isDeepStrictEqual(envelope, withoutUndefined(built)));
check("the envelope includes the schema version", envelope.version === LAYOUT_SCHEMA_VERSION);
check("the envelope omits the lock state", !("locked" in envelope));
check("the envelope includes zones and overrides",
  typeof envelope.zones === "object" && typeof envelope.overrides === "object");
check("the envelope records the zone groups",
  typeof envelope.zones.left === "string" && typeof envelope.zones.right === "string" &&
    typeof envelope.zones.main === "string");

// The status bar is not part of the zone system. main.ts owns the bar;
// mount one here the way the composition root does.
new StatusBar();
check("the status bar is a direct child of body",
  !!window.document.querySelector("body > .status-bar"));
check("the status bar is outside the shell and the dock",
  window.document.querySelector(".ws-shell .status-bar") === null &&
    window.document.querySelector("#dock .status-bar") === null);
check("the status bar never enters the serialized layout",
  !JSON.stringify(envelope.layout).includes("status-bar"));

// --- Reload: a fresh dock restores layout, zones, and panels --------------

const dockEl2 = window.document.createElement("div");
const dock2 = createDock(dockEl2);
initZones(dock2);
check("the persisted layout restores", restoreLayout(dock2, envelope) === true);
await flush();
check("the agent panel is restored through its factory", !!dock2.getPanel(agentPanel.id));
check("the tree panel is restored through its factory", !!dock2.getPanel("tree"));
check("the editor panel is restored through its factory", !!dock2.getPanel(editorAId));
check("restored panels land in their zones",
  zoneOfPanel(dock2.getPanel(agentPanel.id)) === "right" &&
    zoneOfPanel(dock2.getPanel("tree")) === "left" &&
    zoneOfPanel(dock2.getPanel(editorAId)) === "main");
check("the restored editor mounted its surface",
  !!dock2.getPanel(editorAId).view.content.element.querySelector(".cm-editor"));
check("the restored tree keeps its close-button-free tab",
  dock2.getPanel("tree").view.tab.element.querySelector(".dv-default-tab-action") === null);
// The boot anchor guard: re-ensuring the tree after a successful restore
// activates the existing panel instead of duplicating it.
const panelsAfterRestore = dock2.panels.length;
check("ensuring the tree after restore never duplicates it",
  openInZone("tree", {}) === dock2.getPanel("tree") && dock2.panels.length === panelsAfterRestore);

// Debounced writes off onDidLayoutChange, through the writer the
// composition root binds to the workspace bucket. A burst of changes
// inside the debounce window - open B, open C, close C - coalesces into
// one write holding the settled layout: B present, C gone.
const storage = createFakeUiStorage();
startLayoutPersistence(dock2, (value) => storage.set("workspace", KEY, value));
openInZone("editor", { path: FILE_B });
openInZone("editor", { path: FILE_C });
dock2.removePanel(dock2.getPanel(editorCId));
check("no write lands before the debounce elapses", storage.sets.length === 0);
await new Promise((resolve) => setTimeout(resolve, 400));
check("a burst of layout changes coalesces into one write", storage.sets.length === 1);
const debounced = storage.get("workspace", KEY);
check("the debounced write lands on the workspace layout key",
  storage.sets[0]?.bucket === "workspace" && storage.sets[0]?.key === KEY);
check("the debounced write stores the settled layout",
  debounced !== null &&
    Object.keys(debounced.layout.panels).includes(editorBId) &&
    !Object.keys(debounced.layout.panels).includes(editorCId));
check("the debounced write includes the schema version",
  debounced !== null && debounced.version === LAYOUT_SCHEMA_VERSION);
check("the debounced write omits the lock state",
  debounced !== null && !("locked" in debounced));

// A mid-session restore is not a change to save. Open Workspace applies
// the opened file's envelope onto the live dock through
// applyLayoutOrDefault; real Dockview delivers the resulting
// onDidLayoutChange on a microtask, after any synchronous suppression
// around the apply has lifted, and the saver's debounce defers the write
// further still. The saver must drop that echo on its own, then keep
// saving real changes as before.
{
  const setsBefore = storage.sets.length;
  const opened = JSON.parse(JSON.stringify(buildLayoutEnvelope(dock2)));
  applyLayoutOrDefault(dock2, opened);
  await new Promise((resolve) => setTimeout(resolve, 400));
  check("a mid-session restore writes nothing, even past the debounce",
    storage.sets.length === setsBefore);
  openInZone("editor", { path: FILE_C });
  dock2.removePanel(dock2.getPanel(editorCId));
  await new Promise((resolve) => setTimeout(resolve, 400));
  check("a real change after a restore still saves once",
    storage.sets.length === setsBefore + 1);
  check("the save after a restore stores the live layout, not a stale one",
    Object.keys(storage.sets.at(-1).value.layout.panels).includes(editorBId) &&
      !Object.keys(storage.sets.at(-1).value.layout.panels).includes(editorCId));
  await flush();
}

// --- Shortcuts: the dispatcher resolves the contributions' chords -----

// The real contribution surface (imported into the bundle above)
// registered every action and keybinding rule into the shared
// registries; one dispatcher over them replaces the old shortcuts.ts
// listener. Chords resolve through event.code, so each press names
// the physical key's code.
const dispatcher = new KeybindingDispatcher();
const contextKeys = getService(CONTEXT_KEY_SERVICE);
const press = (key, code, options = {}) =>
  window.document.body.dispatchEvent(
    new window.KeyboardEvent("keydown", { key, code, ctrlKey: true, bubbles: true, cancelable: true, ...options }),
  );

// Ctrl+S saves the active editor. The rule's when reads
// editorTextFocus, which the text-control service owns; with no real
// focus traversal in jsdom the test sets the key the way a focused
// CodeMirror surface would.
dock2.getPanel(editorAId).api.setActive();
await flush();
contextKeys.createKey("editorTextFocus", false).set(true);
const putsBeforeSave = puts.length;
press("s", "KeyS");
await flush();
check("Ctrl+S saves the active editor",
  puts.length === putsBeforeSave + 1 && puts.at(-1).path === FILE_A);

// Ctrl+F4 closes the now-clean editor without prompting. The run body
// lazy-imports the command layer, so let the microtasks land.
press("F4", "F4");
await flush();
check("Ctrl+F4 closes the active editor", dock2.getPanel(editorAId) === undefined);
check("a clean close does not prompt",
  window.document.querySelector(".ws-editor-close-overlay") === null);

// Ctrl+Tab / Ctrl+Shift+Tab cycle the editors. Reopen A so two exist.
openInZone("editor", { path: FILE_A });
await flush();
dock2.getPanel(editorBId).api.setActive();
press("Tab", "Tab");
await flush();
check("Ctrl+Tab cycles to the next editor", dock2.activePanel?.id === editorAId);
press("Tab", "Tab", { shiftKey: true });
await flush();
check("Ctrl+Shift+Tab cycles back", dock2.activePanel?.id === editorBId);

// Ctrl+B toggles the Workshop panel.
press("b", "KeyB");
check("Ctrl+B closes the Workshop panel", dock2.getPanel("tree") === undefined);
press("b", "KeyB");
check("Ctrl+B reopens the Workshop panel", !!dock2.getPanel("tree"));
// The reopened tree's chunk resolves asynchronously (the panel registry
// lazy-loads feature directories); let the swap land before focusing.
await flush();

// Ctrl+Shift+E activates and focuses the Workshop tree.
press("e", "KeyE", { shiftKey: true });
const treeContent = dock2.getPanel("tree").view.content;
check("Ctrl+Shift+E activates the Workshop tree", dock2.activePanel?.id === "tree");
check("Ctrl+Shift+E focuses inside the tree",
  treeContent.element.contains(window.document.activeElement));
dispatcher.dispose();

// --- Restore failures fall back to the default layout ---------------------

// A stored value that is not an envelope object at all (the server hands
// back whatever JSON was stored; a string is the closest thing to the old
// corrupt-text case).
const dock3 = createDock(window.document.createElement("div"));
initZones(dock3);
check("a non-object value fails the restore", restoreLayout(dock3, "not json{") === false);
check("an array value fails the restore", restoreLayout(dock3, [envelope]) === false);
openInZone("agent", {});
openInZone("tree", {});
check("the non-object fallback mounts the default layout", dock3.panels.length === 2);

// Stale schema versions are rejected: v1 (the locked-era envelope) and
// v2 (before panels serialized their tabComponent).
const dock4 = createDock(window.document.createElement("div"));
initZones(dock4);
check("a version 1 snapshot fails the restore",
  restoreLayout(dock4, { version: 1, locked: false, zones: {}, overrides: {}, layout: { grid: {} } }) ===
    false);
check("a version 2 snapshot fails the restore",
  restoreLayout(dock4, { version: 2, zones: {}, overrides: {}, layout: { grid: {} } }) === false);

// A structurally valid envelope whose layout fromJSON rejects.
const dock5 = createDock(window.document.createElement("div"));
initZones(dock5);
check("an unloadable layout fails the restore",
  restoreLayout(dock5, {
    version: LAYOUT_SCHEMA_VERSION,
    zones: {},
    overrides: {},
    layout: { grid: { root: { type: "leaf", data: [] } } },
  }) === false);
openInZone("agent", {});
openInZone("tree", {});
check("the unloadable-layout fallback mounts the default layout", dock5.panels.length === 2);

// --- Persistence: a throwing writer is logged, never escapes ---------------

// An uncaught throw inside the debounce timer would take the page down;
// the save logs and the dock stands. Disposing cancels an armed save.
{
  const dockFail = createDock(window.document.createElement("div"));
  initZones(dockFail);
  const errors = [];
  const originalError = console.error;
  console.error = (...args) => {
    errors.push(args);
  };
  const failing = startLayoutPersistence(dockFail, () => {
    throw new Error("denied");
  });
  openInZone("tree", {});
  await new Promise((resolve) => setTimeout(resolve, 400));
  console.error = originalError;
  failing.dispose();
  check("a throwing writer is logged once", errors.length === 1);
  check("a throwing writer leaves the dock intact", dockFail.panels.length === 1);
  let lateWrites = 0;
  const disposed = startLayoutPersistence(dockFail, () => {
    lateWrites += 1;
  });
  openInZone("agent", {});
  disposed.dispose();
  await new Promise((resolve) => setTimeout(resolve, 400));
  check("disposing persistence cancels the armed write", lateWrites === 0);
}

// --- applyLayoutOrDefault: the boot decision main.ts and Open share -------

// Null (no stored layout): the default zones open, tree left and sized,
// agent right, main empty. The fallback resets the zone map itself, so
// the stale group ids the earlier docks left behind are cleared the same
// way a live dock's would be on Open.
{
  const dockDefault = createDock(window.document.createElement("div"));
  initZones(dockDefault);
  let fromJsonCalls = 0;
  const realFromJson = dockDefault.fromJSON.bind(dockDefault);
  dockDefault.fromJSON = (data) => {
    fromJsonCalls += 1;
    return realFromJson(data);
  };
  applyLayoutOrDefault(dockDefault, null);
  await flush();
  check("applyLayoutOrDefault with null never calls fromJSON", fromJsonCalls === 0);
  check("applyLayoutOrDefault with null opens the default zones",
    dockDefault.panels.length === 2 && dockDefault.groups.length === 2);
  check("applyLayoutOrDefault with null places tree left and agent right",
    zoneOfPanel(dockDefault.getPanel("tree")) === "left" &&
      zoneOfPanel(dockDefault.getPanel(panelIdFor("agent", {}))) === "right");
}

// The Open path: the dock already holds the previous workspace's panels
// and the newly opened file has no stored layout. The fallback must
// replace the live arrangement with the default, not layer the anchors
// onto it, so boot and Open share one behavior.
{
  const dockLive = createDock(window.document.createElement("div"));
  initZones(dockLive);
  resetZones();
  openInZone("tree", {});
  openInZone("agent", {});
  openInZone("editor", { path: FILE_A });
  openInZone("editor", { path: FILE_B });
  await flush();
  check("the live dock holds editors before the null apply", dockLive.panels.length === 4);
  applyLayoutOrDefault(dockLive, null);
  await flush();
  check("applyLayoutOrDefault with null clears a live dock's editors",
    dockLive.getPanel(editorAId) === undefined && dockLive.getPanel(editorBId) === undefined);
  check("applyLayoutOrDefault with null leaves a live dock at the default layout",
    dockLive.panels.length === 2 && dockLive.groups.length === 2 &&
      zoneOfPanel(dockLive.getPanel("tree")) === "left" &&
      zoneOfPanel(dockLive.getPanel(panelIdFor("agent", {}))) === "right");
}

// A valid envelope: fromJSON receives it once and the anchors are not
// duplicated by the re-ensure step.
{
  const dockRestore = createDock(window.document.createElement("div"));
  initZones(dockRestore);
  resetZones();
  const received = [];
  const realFromJson = dockRestore.fromJSON.bind(dockRestore);
  dockRestore.fromJSON = (data) => {
    received.push(data);
    return realFromJson(data);
  };
  applyLayoutOrDefault(dockRestore, envelope);
  await flush();
  check("applyLayoutOrDefault with a valid envelope calls fromJSON once with it",
    received.length === 1 && received[0] === envelope.layout);
  check("applyLayoutOrDefault restores every persisted panel without duplicates",
    dockRestore.panels.length === Object.keys(envelope.layout.panels).length &&
      !!dockRestore.getPanel("tree") && !!dockRestore.getPanel(editorAId));
}

// An unloadable envelope: the failed restore clears the dock and the
// default zones take over.
{
  const dockBad = createDock(window.document.createElement("div"));
  initZones(dockBad);
  resetZones();
  applyLayoutOrDefault(dockBad, {
    version: LAYOUT_SCHEMA_VERSION,
    zones: {},
    overrides: {},
    layout: { grid: { root: { type: "leaf", data: [] } } },
  });
  await flush();
  check("applyLayoutOrDefault with an unloadable envelope mounts the default layout",
    dockBad.panels.length === 2 && dockBad.groups.length === 2 && !!dockBad.getPanel("tree"));
}

// --- EditorPanel.requestClose: the dirty close prompt ---------------------

function createStubSurface() {
  const listeners = new Set();
  return {
    element: window.document.createElement("div"),
    currentText: "",
    dirty: false,
    open(document) {
      this.currentText = document.text;
      this.setDirty(false);
    },
    text() {
      return this.currentText;
    },
    markSaved(text) {
      this.setDirty(this.currentText !== text);
    },
    isDirty() {
      return this.dirty;
    },
    onDirtyChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    focus() {},
    dispose() {},
    setDirty(dirty) {
      if (dirty === this.dirty) return;
      this.dirty = dirty;
      for (const listener of listeners) listener(dirty);
    },
    type(text) {
      this.currentText = text;
      this.setDirty(true);
    },
  };
}

function fakeParameters(path, onClose) {
  return { params: { path }, api: { setTitle() {}, close: onClose } };
}

// A clean panel closes without a prompt.
let cleanClosed = false;
const cleanPanel = new EditorPanel({ createSurface: () => createStubSurface() });
cleanPanel.init(fakeParameters(`${ROOT}\\clean.txt`, () => { cleanClosed = true; }));
await flush();
cleanPanel.requestClose();
check("closing a clean editor skips the prompt",
  cleanClosed && cleanPanel.element.querySelector(".ws-editor-close-overlay") === null);

// A dirty panel prompts; Cancel keeps it, Discard closes it.
let dirtyClosed = false;
const dirtyStub = createStubSurface();
const dirtyPanel = new EditorPanel({ createSurface: () => dirtyStub });
dirtyPanel.init(fakeParameters(`${ROOT}\\dirty.txt`, () => { dirtyClosed = true; }));
await flush();
window.document.body.appendChild(dirtyPanel.element);
dirtyStub.type("unsaved\n");
dirtyPanel.requestClose();
const closeOverlay = dirtyPanel.element.querySelector(".ws-editor-close-overlay");
check("closing a dirty editor prompts instead of closing", !dirtyClosed && !!closeOverlay);
check("the close prompt is a modal dialog",
  closeOverlay?.querySelector(".ws-editor-close")?.getAttribute("role") === "dialog" &&
    closeOverlay.querySelector(".ws-editor-close")?.getAttribute("aria-modal") === "true");
const closeButton = (label) =>
  [...dirtyPanel.element.querySelectorAll(".ws-editor-close__button")].find(
    (button) => button.textContent === label,
  );
closeButton("Cancel").click();
check("Cancel keeps the dirty editor open",
  !dirtyClosed && dirtyPanel.element.querySelector(".ws-editor-close-overlay") === null);
check("Cancel leaves the editor dirty", dirtyPanel.isDirty());
dirtyPanel.requestClose();
closeButton("Discard").click();
check("Discard closes the dirty editor", dirtyClosed);

// Save writes, then closes once the write succeeds.
let saveClosed = false;
const saveStub = createStubSurface();
const savePanel = new EditorPanel({ createSurface: () => saveStub });
savePanel.init(fakeParameters(`${ROOT}\\save.txt`, () => { saveClosed = true; }));
await flush();
saveStub.type("keep me\n");
const putsBeforeDialogSave = puts.length;
savePanel.requestClose();
[...savePanel.element.querySelectorAll(".ws-editor-close__button")]
  .find((button) => button.textContent === "Save")
  .click();
await flush();
check("Save writes the dirty editor's text",
  puts.length === putsBeforeDialogSave + 1 && puts.at(-1).text === "keep me\n");
check("Save closes the editor once the write succeeds", saveClosed && !saveStub.isDirty());

if (failures.length > 0) {
  console.error(`workshop-layout: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-layout: all assertions passed");
process.exit(0);
