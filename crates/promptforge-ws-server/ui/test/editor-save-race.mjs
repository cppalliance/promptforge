// Save-race test for the editor panel (src/ui/workshop/editor-panel.ts,
// editor-surface.ts): the saved baseline is the text the write persisted,
// not whatever the editor holds when the PUT resolves. Keystrokes typed
// while a write is in flight must stay dirty - previously markSaved()
// snapshotted the live text, so those keystrokes were baselined as saved
// and requestClose() skipped the unsaved-changes prompt: silent data
// loss. Drives the real EditorPanel with a stubbed surface and a writer
// that stalls until the test releases it.
// Run: node test/editor-save-race.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { EditorPanel } from "./src/ui/workshop/editor-panel.ts";
      export { ModifiedConflictError } from "./src/services/workspace-api.ts";
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
  // The panel imports its colocated CSS; strip it - the test drives only
  // the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;

for (const key of [
  "document",
  "navigator",
  "HTMLElement",
  "Node",
  "Element",
  "Event",
  "CustomEvent",
  "KeyboardEvent",
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

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-save-race-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, EditorPanel, ModifiedConflictError } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const FILE_PATH = "C:\\project\\race.txt";

// The panel's contract stub, mirroring markSaved's semantics: the saved
// baseline is the written text, dirty recomputes against the live text.
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

// A writer that records every PUT and stalls each one until released.
function createStallingWriter() {
  const puts = [];
  let release = null;
  let nextModifiedMs = 200;
  return {
    puts,
    write(filePath, text, expectedModifiedMs) {
      puts.push({ path: filePath, text, expectedModifiedMs });
      return new Promise((resolve) => {
        release = () => {
          const modifiedMs = nextModifiedMs;
          nextModifiedMs += 100;
          resolve({ path: filePath, size: text.length, modifiedMs, text });
        };
      });
    },
    release() {
      release();
      release = null;
    },
  };
}

function fakeParameters(filePath, onClose) {
  return { params: { path: filePath }, api: { setTitle() {}, close: onClose ?? (() => {}) } };
}

const readFile = async () => ({ path: FILE_PATH, size: 0, modifiedMs: 100, text: "" });

await assertNoLeaks(lifecycle, async () => {
  // --- Typing while a save is in flight stays dirty -------------------------

  const stub = createStubSurface();
  const writer = createStallingWriter();
  const panel = new EditorPanel({
    createSurface: () => stub,
    readFile,
    writeFile: writer.write,
  });
  panel.init(fakeParameters(FILE_PATH));
  await flush();

  stub.type("A");
  const firstSave = panel.save();
  check("the stalled save writes the text captured at save time", writer.puts.at(-1)?.text === "A");

  void panel.save();
  check("a second save while one is in flight is a no-op", writer.puts.length === 1);

  stub.type("AB");
  writer.release();
  await firstSave;
  await flush();

  check("a keystroke typed during the write stays dirty after the save resolves", panel.isDirty());

  const secondSave = panel.save();
  check(
    "a second save writes the full text including the in-flight keystroke",
    writer.puts.length === 2 && writer.puts.at(-1).text === "AB",
  );
  check(
    "the second save carries the token from the first write",
    writer.puts.at(-1).expectedModifiedMs === 200,
  );
  writer.release();
  await secondSave;
  check("the second save clears the dirty state", !panel.isDirty());
  panel.dispose();

  // --- The close-dialog Save path keeps the panel open while text is unsaved

  let closed = false;
  const closeStub = createStubSurface();
  const closeWriter = createStallingWriter();
  const closePanel = new EditorPanel({
    createSurface: () => closeStub,
    readFile,
    writeFile: closeWriter.write,
  });
  closePanel.init(fakeParameters(FILE_PATH, () => {
    closed = true;
  }));
  await flush();

  closeStub.type("A");
  closePanel.requestClose();
  check(
    "a dirty panel opens the unsaved-changes dialog",
    closePanel.element.querySelector(".editor-close-overlay") !== null,
  );
  const saveButton = [...closePanel.element.querySelectorAll(".editor-close__button")].find(
    (button) => button.textContent === "Save",
  );
  saveButton.click();
  closeStub.type("AB");
  closeWriter.release();
  await flush();

  check("the close-dialog Save does not close while a keystroke is unsaved", !closed);
  check("the panel is still dirty after the close-dialog Save", closePanel.isDirty());

  const finishSave = closePanel.save();
  closeWriter.release();
  await finishSave;
  closePanel.requestClose();
  check("once fully saved, the panel closes without a prompt", closed);
  closePanel.dispose();

  // --- The saving guard does not wedge the conflict dialog's Overwrite ------

  const conflictStub = createStubSurface();
  const conflictPuts = [];
  let releaseOverwrite = null;
  let failNextPut = true;
  const conflictPanel = new EditorPanel({
    createSurface: () => conflictStub,
    readFile: async () => ({ path: FILE_PATH, size: 7, modifiedMs: 500, text: "on disk" }),
    writeFile: (filePath, text, expectedModifiedMs) => {
      if (failNextPut) {
        failNextPut = false;
        return Promise.reject(new ModifiedConflictError("file changed on disk"));
      }
      conflictPuts.push({ path: filePath, text, expectedModifiedMs });
      return new Promise((resolve) => {
        releaseOverwrite = () =>
          resolve({ path: filePath, size: text.length, modifiedMs: 900, text });
      });
    },
  });
  conflictPanel.init(fakeParameters(FILE_PATH));
  await flush();

  conflictStub.type("mine");
  await conflictPanel.save();
  check(
    "a conflicted save opens the conflict dialog",
    conflictPanel.element.querySelector(".editor-conflict-overlay") !== null,
  );

  const overwriteButton = [...conflictPanel.element.querySelectorAll(".editor-conflict__button")]
    .find((button) => button.textContent === "Overwrite");
  overwriteButton.click();
  await flush();
  check(
    "the Overwrite button runs despite the saving guard, with the fresh token",
    conflictPuts.length === 1 && conflictPuts[0].expectedModifiedMs === 500,
  );

  void conflictPanel.save();
  check("a save while the overwrite is in flight is a no-op", conflictPuts.length === 1);

  releaseOverwrite();
  await flush();
  check("the released overwrite clears the dirty state", !conflictPanel.isDirty());
  conflictPanel.dispose();
});

if (failures.length > 0) {
  console.error(`editor-save-race: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-save-race: all assertions passed");
process.exit(0);
