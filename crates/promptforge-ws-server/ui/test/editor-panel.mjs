// Integration test for the editor panel and EditorSurface contract
// (src/ui/workshop/editor-panel.ts, editor-surface.ts, and the file
// read/write half of src/services/workspace-api.ts). Bundles the modules with esbuild
// and drives them in jsdom.
//
// jsdom note: CodeMirror 6 runs in jsdom once Range gains the measurement
// shims below (getBoundingClientRect / getClientRects), so the surface
// contract - open, dirty tracking, markSaved - is tested against the real
// CodeMirrorSurface. The panel logic (dirty title, save flow, conflict
// dialog) is tested with the real EditorPanel and a stubbed surface, so
// those assertions never depend on editor internals.
//
// Run: node test/editor-panel.mjs
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
      export { EditorPanel } from "./src/ui/workshop/editor-panel.ts";
      export { CodeMirrorSurface } from "./src/ui/workshop/editor-surface.ts";
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

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
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
  "Window",
  "HTMLElement",
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

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-panel-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { EditorPanel, CodeMirrorSurface } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- A scripted workspace file API with a token state machine -------------
//
// The "disk" holds text and a modified-time token. PUT with a matching
// expected token writes and bumps the token; a stale token answers 409
// with the server's modified_conflict code. `failNextPut` forces one
// conflict regardless, so the test can arm the race deterministically.
const FILE_PATH = "C:\\project\\a.txt";
const disk = { text: "hello\n", modifiedMs: 100, failNextPut: false };
const puts = [];
globalThis.fetch = async (url, options) => {
  const target = typeof url === "string" ? url : url.url;
  if (target.startsWith("/workspace/file") && !options) {
    return {
      ok: true,
      status: 200,
      json: async () => ({
        path: FILE_PATH,
        size: disk.text.length,
        modified_ms: disk.modifiedMs,
        text: disk.text,
      }),
    };
  }
  if (target === "/workspace/file" && options?.method === "PUT") {
    const body = JSON.parse(options.body);
    puts.push(body);
    if (disk.failNextPut || body.expected_modified_ms !== disk.modifiedMs) {
      disk.failNextPut = false;
      return {
        ok: false,
        status: 409,
        json: async () => ({
          error: { message: "file changed on disk since it was read", code: "modified_conflict" },
        }),
      };
    }
    disk.text = body.text;
    disk.modifiedMs += 100;
    return {
      ok: true,
      status: 200,
      json: async () => ({
        path: FILE_PATH,
        size: disk.text.length,
        modified_ms: disk.modifiedMs,
        text: disk.text,
      }),
    };
  }
  throw new Error(`unexpected fetch in the editor-panel test: ${target}`);
};

// --- A stub EditorSurface: the panel's contract, no editor internals ------
function createStubSurface() {
  const listeners = new Set();
  return {
    element: window.document.createElement("div"),
    opened: [],
    currentText: "",
    dirty: false,
    open(document) {
      this.opened.push(document);
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
    setReadOnly() {},
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

function fakeParameters(path) {
  const titles = [];
  return {
    params: { path },
    api: { setTitle: (title) => titles.push(title) },
    titles,
  };
}

// --- Panel logic: open, dirty title, save, conflict dialog -----------------

const stub = createStubSurface();
const params = fakeParameters(FILE_PATH);
const panel = new EditorPanel({ createSurface: () => stub });
panel.init(params);
await flush();

check(
  "opening a file loads its text into the surface",
  stub.opened.length === 1 && stub.opened[0].text === "hello\n",
);
check("the tab title is the file's base name", params.titles.at(-1) === "a.txt");
check("a freshly opened document is not dirty", !panel.isDirty());

stub.type("hello world\n");
check("typing sets the dirty state", panel.isDirty());
check("dirty state shows in the panel title", params.titles.at(-1) === "● a.txt");

await panel.save();
check("save writes with the read token", puts.length === 1 && puts[0].expected_modified_ms === 100);
check("save sends the editor text", puts[0].text === "hello world\n");
check("save clears the dirty state", !panel.isDirty());
check("save restores the clean title", params.titles.at(-1) === "a.txt");

// Stale token: the next PUT conflicts, surfacing the dialog.
stub.type("local edits\n");
disk.failNextPut = true;
await panel.save();
const overlay = panel.element.querySelector(".editor-conflict-overlay");
check("a stale modified-time token surfaces the conflict dialog", !!overlay);
check(
  "the conflict dialog is a modal dialog",
  overlay?.querySelector(".editor-conflict")?.getAttribute("role") === "dialog" &&
    overlay.querySelector(".editor-conflict")?.getAttribute("aria-modal") === "true",
);
check("the conflicted save did not write", disk.text === "hello world\n");

// Escape dismisses without resolving the conflict.
document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape" }));
check("Escape dismisses the conflict dialog", !panel.element.querySelector(".editor-conflict-overlay"));
check("dismissing leaves the editor dirty", panel.isDirty());

// Reload: the on-disk text replaces the editor's text.
disk.text = "on disk\n";
disk.failNextPut = true;
await panel.save();
const reloadButton = [...panel.element.querySelectorAll(".editor-conflict__button")].find(
  (button) => button.textContent === "Reload",
);
reloadButton.click();
await flush();
check("Reload dismisses the dialog", !panel.element.querySelector(".editor-conflict-overlay"));
check(
  "Reload replaces the editor text with the on-disk text",
  stub.opened.at(-1).text === "on disk\n",
);
check("Reload clears the dirty state", !panel.isDirty());

// Overwrite: re-read the fresh token, then write the editor's text.
stub.type("mine\n");
disk.failNextPut = true;
await panel.save();
const overwriteButton = [...panel.element.querySelectorAll(".editor-conflict__button")].find(
  (button) => button.textContent === "Overwrite",
);
const putsBeforeOverwrite = puts.length;
overwriteButton.click();
await flush();
check("Overwrite dismisses the dialog", !panel.element.querySelector(".editor-conflict-overlay"));
check(
  "Overwrite re-reads the token and writes the editor text",
  puts.length === putsBeforeOverwrite + 1 &&
    puts.at(-1).text === "mine\n" &&
    puts.at(-1).expected_modified_ms === disk.modifiedMs - 100,
);
check("Overwrite wins the file", disk.text === "mine\n");
check("Overwrite clears the dirty state", !panel.isDirty());

// The conflict dialog traps Tab inside its buttons (jsdom only tracks
// focus for connected elements, so the panel mounts for this pass).
stub.type("trap check\n");
disk.failNextPut = true;
await panel.save();
window.document.body.appendChild(panel.element);
const trapButtons = [...panel.element.querySelectorAll(".editor-conflict__button")];
trapButtons.at(-1).focus();
document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab" }));
check("Tab wraps focus from the last button to the first", document.activeElement === trapButtons[0]);
trapButtons[0].focus();
document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true }));
check(
  "Shift+Tab wraps focus from the first button to the last",
  document.activeElement === trapButtons.at(-1),
);
document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape" }));
panel.element.remove();

// --- The real CodeMirrorSurface honors the EditorSurface contract ---------

const surface = new CodeMirrorSurface();
window.document.body.appendChild(surface.element);
surface.open({ path: FILE_PATH, text: "one\n" });
check("the real surface mounts a CodeMirror editor", !!surface.element.querySelector(".cm-editor"));
check("the real surface starts clean", !surface.isDirty());
check("the real surface reports its text", surface.text() === "one\n");

// Type through the view's dispatch, as user input would. The view is
// TypeScript-private but reachable at runtime, which keeps the dirty
// assertions on the real updateListener instead of a reimplementation.
const view = surface.element.querySelector(".cm-content");
check("the editor content is editable", view?.getAttribute("contenteditable") === "true");

const dirtyEvents = [];
const unsubscribe = surface.onDirtyChange((dirty) => dirtyEvents.push(dirty));
surface.view.dispatch({ changes: { from: 3, insert: "!" } });
check("an edit sets the real surface dirty", surface.isDirty());
check("the dirty transition fires the listener", dirtyEvents.join(",") === "true");
check("the edit landed in the document", surface.text() === "one!\n");

// Reverting the edit by hand returns to the saved baseline: clean again.
surface.view.dispatch({ changes: { from: 3, to: 4 } });
check("reverting to the saved text clears dirty", !surface.isDirty());
check("the clean transition fires the listener", dirtyEvents.join(",") === "true,false");

surface.view.dispatch({ changes: { from: 0, insert: "x" } });
check("a second edit dirties again", surface.isDirty());
surface.markSaved();
check("markSaved clears dirty and rebaselines", !surface.isDirty());
surface.view.dispatch({ changes: { from: 1, to: 1 } });
check("a no-op change keeps the surface clean", !surface.isDirty());

surface.open({ path: FILE_PATH, text: "two\n" });
check("reopening replaces the document", surface.text() === "two\n" && !surface.isDirty());
unsubscribe();
surface.dispose();
check("dispose tears the editor down", !surface.element.querySelector(".cm-editor"));

if (failures.length > 0) {
  console.error(`editor-panel: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-panel: all assertions passed");
process.exit(0);
