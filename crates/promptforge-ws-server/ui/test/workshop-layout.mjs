// Integration test for layout boot, lock, persistence, and shortcuts
// (src/workshop/layout-lock.ts, layout-persistence.ts, shortcuts.ts, the
// zone-state serialization in zones.ts, and EditorPanel.requestClose).
// Bundles the modules with esbuild, mounts real Dockview docks in jsdom
// against the real index.html, and drives the public API. Covers: the
// lock starts engaged; unlocking calls updateOptions and reveals the
// gated affordances; the layout survives a reload (serialize -> restore);
// corrupt, version-mismatched, and unloadable layouts fall back to
// defaults; each shortcut dispatches its command; the status bar never
// enters the serialized layout. Run: node test/workshop-layout.mjs
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
        openAgentPanel,
        openInZone,
        panelIdFor,
        zoneOfPanel,
      } from "./src/workshop/zones.ts";
      export { createPanelComponent } from "./src/workshop/panel-types.ts";
      export {
        initLayoutLock,
        isLayoutLocked,
        setLayoutLocked,
        createLockHeaderControl,
      } from "./src/workshop/layout-lock.ts";
      export {
        restoreLayout,
        persistLayout,
        startLayoutPersistence,
        LAYOUT_STORAGE_KEY,
      } from "./src/workshop/layout-persistence.ts";
      export { installShortcuts } from "./src/workshop/shortcuts.ts";
      export { EditorPanel } from "./src/workshop/editor-panel.ts";
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
// modified-time token. Any other route fails the test loudly.
const ROOT = "C:\\project";
const FILE_A = `${ROOT}\\a.txt`;
const FILE_B = `${ROOT}\\b.txt`;
const puts = [];
let token = 100;
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
          modified_ms: token,
          text: `text of ${pathParam}`,
        }),
      };
    }
  }
  if (target === "/workspace/file" && options?.method === "PUT") {
    const body = JSON.parse(options.body);
    puts.push(body);
    token += 100;
    return {
      ok: true,
      status: 200,
      json: async () => ({
        path: body.path,
        size: body.text.length,
        modified_ms: token,
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
        entries: [{ name: "project", path: ROOT, kind: "directory", size: 0, modified_ms: 100 }],
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

const bundlePath = path.join(os.tmpdir(), "promptforge-workshop-layout-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  initZones,
  openAgentPanel,
  openInZone,
  panelIdFor,
  zoneOfPanel,
  createPanelComponent,
  initLayoutLock,
  isLayoutLocked,
  setLayoutLocked,
  createLockHeaderControl,
  restoreLayout,
  persistLayout,
  startLayoutPersistence,
  LAYOUT_STORAGE_KEY,
  installShortcuts,
  EditorPanel,
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

window.localStorage.clear();

// Builds a dock wired exactly as main.ts wires it, on a fresh element.
function createDock(element) {
  element.className = "dock";
  window.document.body.appendChild(element);
  return createDockview(element, {
    createComponent: createPanelComponent,
    createRightHeaderActionComponent: createLockHeaderControl,
    theme: themeDark,
    singleTabMode: "fullwidth",
    disableFloatingGroups: true,
    hideBorders: true,
    locked: true,
    noPanelsOverlay: "emptyGroup",
  });
}

const editorAId = panelIdFor("editor", { path: FILE_A });
const editorBId = panelIdFor("editor", { path: FILE_B });

// --- Boot: the lock starts engaged, the default layout is three-zone ------

const dockEl = window.document.getElementById("dock");
const updateCalls = [];
const dock = createDockview(dockEl, {
  createComponent: createPanelComponent,
  createRightHeaderActionComponent: createLockHeaderControl,
  theme: themeDark,
  singleTabMode: "fullwidth",
  disableFloatingGroups: true,
  hideBorders: true,
  locked: true,
  noPanelsOverlay: "emptyGroup",
});
const originalUpdate = dock.updateOptions.bind(dock);
dock.updateOptions = (options) => {
  updateCalls.push(options);
  return originalUpdate(options);
};
initZones(dock);
initLayoutLock(dock, dockEl);

check("the layout lock starts engaged", isLayoutLocked());
check("the dock carries the locked class", dockEl.classList.contains("dock--locked"));
check("empty storage has nothing to restore", restoreLayout(dock) === false);

const chatPanel = openAgentPanel();
const treePanel = openInZone("tree", {});
treePanel.group.api.setSize({ width: 280 });
await flush();

check("the default layout mounts chat and tree", dock.panels.length === 2);
check("main stays empty until a document opens", dock.groups.length === 2);
check("chat opens right, the tree left", zoneOfPanel(chatPanel) === "right" && zoneOfPanel(treePanel) === "left");
check(
  "every zone header carries a lock control",
  window.document.querySelectorAll("#dock .layout-lock-toggle").length === 2,
);

// The lock never blocks app placement: an editor opens while locked.
const editorA = openInZone("editor", { path: FILE_A });
await flush();
check("app placement works while locked", zoneOfPanel(editorA) === "main");

// --- Unlocking: updateOptions fires and the gated class lifts -------------

const lockButton = window.document.querySelector("#dock .layout-lock-toggle");
check("the lock control announces itself", lockButton?.getAttribute("aria-label") === "Unlock layout");
lockButton.click();
check("the header control unlocks the layout", !isLayoutLocked());
check("unlocking calls dock.updateOptions({ locked: false })",
  updateCalls.some((options) => options.locked === false));
check("unlocking lifts the locked class, revealing the tab affordances",
  !dockEl.classList.contains("dock--locked"));
check("the control reflects the unlocked state",
  lockButton.getAttribute("aria-label") === "Lock layout");
setLayoutLocked(true);
check("relocking restores the locked class", dockEl.classList.contains("dock--locked"));

// --- Persistence: the envelope is versioned and carries placement ---------

persistLayout(dock);
const raw = window.localStorage.getItem(LAYOUT_STORAGE_KEY);
check("the layout persists to localStorage", raw !== null);
const envelope = JSON.parse(raw);
check("the envelope carries the schema version", envelope.version === 1);
check("the envelope carries the lock state", envelope.locked === true);
check("the envelope carries zones and overrides",
  typeof envelope.zones === "object" && typeof envelope.overrides === "object");
check("the envelope records the zone groups",
  typeof envelope.zones.left === "string" && typeof envelope.zones.right === "string" &&
    typeof envelope.zones.main === "string");

// The status bar is not part of the zone system.
check("the status bar is a direct child of body",
  !!window.document.querySelector("body > .status-bar"));
check("the status bar is outside the shell and the dock",
  window.document.querySelector(".shell .status-bar") === null &&
    window.document.querySelector("#dock .status-bar") === null);
check("the status bar never enters the serialized layout",
  !JSON.stringify(envelope.layout).includes("status-bar"));

// --- Reload: a fresh dock restores layout, zones, and panels --------------

const dockEl2 = window.document.createElement("div");
const dock2 = createDock(dockEl2);
initZones(dock2);
initLayoutLock(dock2, dockEl2);
check("the persisted layout restores", restoreLayout(dock2) === true);
await flush();
check("the agent panel is restored through its factory", !!dock2.getPanel(chatPanel.id));
check("the tree panel is restored through its factory", !!dock2.getPanel("tree"));
check("the editor panel is restored through its factory", !!dock2.getPanel(editorAId));
check("restored panels land in their zones",
  zoneOfPanel(dock2.getPanel(chatPanel.id)) === "right" &&
    zoneOfPanel(dock2.getPanel("tree")) === "left" &&
    zoneOfPanel(dock2.getPanel(editorAId)) === "main");
check("the restored editor mounted its surface",
  !!dock2.getPanel(editorAId).view.content.element.querySelector(".cm-editor"));

// Debounced writes off onDidLayoutChange; immediate writes on lock toggle.
startLayoutPersistence(dock2);
window.localStorage.removeItem(LAYOUT_STORAGE_KEY);
openInZone("editor", { path: FILE_B });
await new Promise((resolve) => setTimeout(resolve, 400));
const debounced = window.localStorage.getItem(LAYOUT_STORAGE_KEY);
check("layout changes persist debounced", debounced !== null);
check("the debounced write carries the new panel",
  debounced !== null &&
    Object.keys(JSON.parse(debounced).layout.panels).includes(editorBId));
window.localStorage.removeItem(LAYOUT_STORAGE_KEY);
setLayoutLocked(false);
const lockWrite = window.localStorage.getItem(LAYOUT_STORAGE_KEY);
check("a lock toggle persists immediately", lockWrite !== null);
check("the lock write carries the new lock state",
  lockWrite !== null && JSON.parse(lockWrite).locked === false);
setLayoutLocked(true);

// --- Shortcuts: one keydown listener dispatching to the commands ----------

const uninstall = installShortcuts(dock2);
const press = (key, options = {}) =>
  window.document.dispatchEvent(
    new window.KeyboardEvent("keydown", { key, ctrlKey: true, cancelable: true, ...options }),
  );

// Ctrl+S saves the active editor.
dock2.getPanel(editorAId).api.setActive();
await flush();
const putsBeforeSave = puts.length;
press("s");
await flush();
check("Ctrl+S saves the active editor",
  puts.length === putsBeforeSave + 1 && puts.at(-1).path === FILE_A);

// Ctrl+W closes the now-clean editor without prompting.
press("w");
check("Ctrl+W closes the active editor", dock2.getPanel(editorAId) === undefined);
check("a clean close does not prompt",
  window.document.querySelector(".editor-close-overlay") === null);

// Ctrl+Tab / Ctrl+Shift+Tab cycle the editors. Reopen A so two exist.
openInZone("editor", { path: FILE_A });
await flush();
dock2.getPanel(editorBId).api.setActive();
press("Tab");
check("Ctrl+Tab cycles to the next editor", dock2.activePanel?.id === editorAId);
press("Tab", { shiftKey: true });
check("Ctrl+Shift+Tab cycles back", dock2.activePanel?.id === editorBId);

// Ctrl+B toggles the Workshop panel.
press("b");
check("Ctrl+B closes the Workshop panel", dock2.getPanel("tree") === undefined);
press("b");
check("Ctrl+B reopens the Workshop panel", !!dock2.getPanel("tree"));

// Ctrl+Shift+F activates and focuses the Workshop tree.
press("f", { shiftKey: true });
const treeContent = dock2.getPanel("tree").view.content;
check("Ctrl+Shift+F activates the Workshop tree", dock2.activePanel?.id === "tree");
check("Ctrl+Shift+F focuses inside the tree",
  treeContent.element.contains(window.document.activeElement));
uninstall();

// --- Restore failures fall back to the default layout ---------------------

// Corrupt JSON.
window.localStorage.setItem(LAYOUT_STORAGE_KEY, "not json{");
const dock3 = createDock(window.document.createElement("div"));
initZones(dock3);
check("corrupt storage fails the restore", restoreLayout(dock3) === false);
openAgentPanel();
openInZone("tree", {});
check("the corrupt-storage fallback mounts the default layout", dock3.panels.length === 2);

// A schema version mismatch.
window.localStorage.setItem(
  LAYOUT_STORAGE_KEY,
  JSON.stringify({ version: 999, locked: false, zones: {}, overrides: {}, layout: { grid: {} } }),
);
const dock4 = createDock(window.document.createElement("div"));
initZones(dock4);
check("a version mismatch fails the restore", restoreLayout(dock4) === false);

// A structurally valid envelope whose layout fromJSON rejects.
window.localStorage.setItem(
  LAYOUT_STORAGE_KEY,
  JSON.stringify({
    version: 1,
    locked: false,
    zones: {},
    overrides: {},
    layout: { grid: { root: { type: "leaf", data: [] } } },
  }),
);
const dock5 = createDock(window.document.createElement("div"));
initZones(dock5);
check("an unloadable layout fails the restore", restoreLayout(dock5) === false);
openAgentPanel();
openInZone("tree", {});
check("the unloadable-layout fallback mounts the default layout", dock5.panels.length === 2);
window.localStorage.removeItem(LAYOUT_STORAGE_KEY);

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
    markSaved() {
      this.setDirty(false);
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
  cleanClosed && cleanPanel.element.querySelector(".editor-close-overlay") === null);

// A dirty panel prompts; Cancel keeps it, Discard closes it.
let dirtyClosed = false;
const dirtyStub = createStubSurface();
const dirtyPanel = new EditorPanel({ createSurface: () => dirtyStub });
dirtyPanel.init(fakeParameters(`${ROOT}\\dirty.txt`, () => { dirtyClosed = true; }));
await flush();
window.document.body.appendChild(dirtyPanel.element);
dirtyStub.type("unsaved\n");
dirtyPanel.requestClose();
const closeOverlay = dirtyPanel.element.querySelector(".editor-close-overlay");
check("closing a dirty editor prompts instead of closing", !dirtyClosed && !!closeOverlay);
check("the close prompt is a modal dialog",
  closeOverlay?.querySelector(".editor-close")?.getAttribute("role") === "dialog" &&
    closeOverlay.querySelector(".editor-close")?.getAttribute("aria-modal") === "true");
const closeButton = (label) =>
  [...dirtyPanel.element.querySelectorAll(".editor-close__button")].find(
    (button) => button.textContent === label,
  );
closeButton("Cancel").click();
check("Cancel keeps the dirty editor open",
  !dirtyClosed && dirtyPanel.element.querySelector(".editor-close-overlay") === null);
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
[...savePanel.element.querySelectorAll(".editor-close__button")]
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
