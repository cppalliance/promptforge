// Save-timeout test for the editor panel (src/parts/editor/editor-panel.ts):
// a 408 on save (the deadline_elapsed error) leaves the conflict token
// unknown, and the next save reconciles with the file on disk instead of
// re-sending a token that may now be stale. Covers six cases: the 408
// marks the token unknown and sends no stale token; a disk match adopts
// the fresh token and saves; a mismatch shows the conflict dialog; a
// late write that lands after the re-read surfaces the conflict dialog,
// not a raw error; an Overwrite 408 marks the token unknown the same way
// a save 408 does; and an Overwrite entered with the token already
// unknown, whose timed-out write landed, lets the next save adopt the
// fresh token. Drives the real EditorPanel with a stubbed surface and
// scripted reader/writer, the same way editor-save-race.mjs does.
// Run: node test/editor-save-timeout.mjs
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
      export { EditorPanel } from "./src/parts/editor/editor-panel.ts";
      export { CatalogError, ErrorCatalog } from "./src/services/error-catalog.ts";
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
  url: "http://127.0.0.1:7913/",
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

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-save-timeout-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, EditorPanel, CatalogError, ErrorCatalog } = await import(
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

const FILE_PATH = "C:\\project\\save-timeout.txt";

// The panel's contract stub, mirroring editor-save-race.mjs: markSaved
// rebaselines against the written text, dirty recomputes against the live
// text.
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

function fakeParameters(filePath) {
  return { params: { path: filePath }, api: { setTitle() {}, close() {} } };
}

// The typed failures the writer surfaces, matching the write boundary's
// codes so the panel's narrowing helpers recognize them.
const deadlineError = () =>
  new CatalogError(ErrorCatalog.DeadlineElapsed, "save timed out", { status: 408 });
const conflictError = () =>
  new CatalogError(ErrorCatalog.ModifiedConflict, "file changed on disk", { status: 409 });

const errorBar = (panel) => panel.element.querySelector(".ws-editor-panel__error");
const conflictOverlay = (panel) => panel.element.querySelector(".ws-editor-conflict-overlay");
const overwriteButton = (panel) =>
  [...panel.element.querySelectorAll(".ws-editor-conflict__button")].find(
    (button) => button.textContent === "Overwrite",
  );

await assertNoLeaks(lifecycle, async () => {
  // --- A 408 leaves the token unknown and sends no stale token ---------------
  {
    const stub = createStubSurface();
    const puts = [];
    let disk = { text: "old", token: "t100" };
    const panel = new EditorPanel({
      createSurface: () => stub,
      readFile: async () => ({
        path: FILE_PATH, size: disk.text.length, token: disk.token, text: disk.text,
      }),
      writeFile: (filePath, text, expectedToken) => {
        puts.push({ path: filePath, text, expectedToken });
        // The write never lands: the server answered 408 before it wrote.
        return Promise.reject(deadlineError());
      },
    });
    panel.init(fakeParameters(FILE_PATH));
    await flush();

    stub.type("new");
    await panel.save();
    check(
      "a 408 tells the user the save may not have landed",
      errorBar(panel)?.textContent.includes("may or may not"),
    );

    await panel.save();
    check(
      "an unknown token never sends the stale token on the next save",
      puts.length === 1 && puts[0].expectedToken === "t100",
    );
    panel.dispose();
  }

  // --- A disk match adopts the fresh token and saves -------------------------
  {
    const stub = createStubSurface();
    const puts = [];
    let disk = { text: "old", token: "t100" };
    let timedOut = true;
    const panel = new EditorPanel({
      createSurface: () => stub,
      readFile: async () => ({
        path: FILE_PATH, size: disk.text.length, token: disk.token, text: disk.text,
      }),
      writeFile: (filePath, text, expectedToken) => {
        puts.push({ path: filePath, text, expectedToken });
        if (timedOut) {
          timedOut = false;
          // The late write lands: the disk now holds what was sent.
          disk = { text, token: "t200" };
          return Promise.reject(deadlineError());
        }
        return Promise.resolve({ path: filePath, size: text.length, token: "t300", text });
      },
    });
    panel.init(fakeParameters(FILE_PATH));
    await flush();

    stub.type("new");
    await panel.save();
    await panel.save();
    check(
      "a disk match adopts the fresh token and saves with it",
      puts.length === 2 && puts[1].expectedToken === "t200" && puts[1].text === "new",
    );
    check("the adopted-token save clears the dirty state", !panel.isDirty());
    panel.dispose();
  }

  // --- A mismatch shows the conflict dialog ----------------------------------
  {
    const stub = createStubSurface();
    const puts = [];
    let disk = { text: "old", token: "t100" };
    const panel = new EditorPanel({
      createSurface: () => stub,
      readFile: async () => ({
        path: FILE_PATH, size: disk.text.length, token: disk.token, text: disk.text,
      }),
      writeFile: (filePath, text, expectedToken) => {
        puts.push({ path: filePath, text, expectedToken });
        return Promise.reject(deadlineError());
      },
    });
    panel.init(fakeParameters(FILE_PATH));
    await flush();

    stub.type("new");
    await panel.save();
    // The file changed externally while the write was unknown.
    disk = { text: "external edit", token: "t500" };
    await panel.save();
    check("a mismatched disk shows the conflict dialog", conflictOverlay(panel) !== null);
    check("a mismatch never writes with the stale token", puts.length === 1);
    panel.dispose();
  }

  // --- A late write landing after the re-read shows the conflict dialog -------
  {
    const stub = createStubSurface();
    const puts = [];
    let disk = { text: "old", token: "t100" };
    let timedOut = true;
    const panel = new EditorPanel({
      createSurface: () => stub,
      readFile: async () => ({
        path: FILE_PATH, size: disk.text.length, token: disk.token, text: disk.text,
      }),
      writeFile: (filePath, text, expectedToken) => {
        puts.push({ path: filePath, text, expectedToken });
        if (timedOut) {
          timedOut = false;
          // The late write lands before the re-read, so the disk matches.
          disk = { text, token: "t200" };
          return Promise.reject(deadlineError());
        }
        // The adopted token was fresh at re-read time, but a late write
        // landed afterward and bumped the token: the write conflicts.
        return Promise.reject(conflictError());
      },
    });
    panel.init(fakeParameters(FILE_PATH));
    await flush();

    stub.type("new");
    await panel.save();
    errorBar(panel)?.remove();
    await panel.save();
    check(
      "a late write landing after the re-read shows the conflict dialog",
      conflictOverlay(panel) !== null,
    );
    check(
      "a late write landing after the re-read is not a raw error",
      errorBar(panel) === null,
    );
    panel.dispose();
  }

  // --- An Overwrite 408 leaves the token unknown ------------------------------
  {
    const stub = createStubSurface();
    const puts = [];
    let disk = { text: "old", token: "t100" };
    const panel = new EditorPanel({
      createSurface: () => stub,
      readFile: async () => ({
        path: FILE_PATH, size: disk.text.length, token: disk.token, text: disk.text,
      }),
      writeFile: (filePath, text, expectedToken) => {
        puts.push({ path: filePath, text, expectedToken });
        if (puts.length === 1) {
          // The file changed externally since the load: the save conflicts.
          disk = { text: "external edit", token: "t500" };
          return Promise.reject(conflictError());
        }
        if (puts.length === 2) {
          // The Overwrite never lands: the server answered 408 before it wrote.
          return Promise.reject(deadlineError());
        }
        return Promise.resolve({ path: filePath, size: text.length, token: "t900", text });
      },
    });
    panel.init(fakeParameters(FILE_PATH));
    await flush();

    stub.type("new");
    await panel.save();
    overwriteButton(panel).click();
    await flush();
    check(
      "an Overwrite 408 tells the user the save may not have landed",
      errorBar(panel)?.textContent.includes("may or may not"),
    );

    await panel.save();
    check(
      "an Overwrite 408 leaves the token unknown, so the next save re-reads instead of writing",
      puts.length === 2 && conflictOverlay(panel) !== null,
    );
    panel.dispose();
  }

  // --- An Overwrite whose timed-out write landed adopts the fresh token -------
  {
    const stub = createStubSurface();
    const puts = [];
    let disk = { text: "old", token: "t100" };
    const panel = new EditorPanel({
      createSurface: () => stub,
      readFile: async () => ({
        path: FILE_PATH, size: disk.text.length, token: disk.token, text: disk.text,
      }),
      writeFile: (filePath, text, expectedToken) => {
        puts.push({ path: filePath, text, expectedToken });
        if (puts.length === 1) {
          // The save never lands: the server answered 408 before it wrote.
          return Promise.reject(deadlineError());
        }
        if (puts.length === 2) {
          // The late Overwrite lands: the disk now holds what was sent.
          disk = { text, token: "t600" };
          return Promise.reject(deadlineError());
        }
        return Promise.resolve({ path: filePath, size: text.length, token: "t700", text });
      },
    });
    panel.init(fakeParameters(FILE_PATH));
    await flush();

    stub.type("new");
    await panel.save();
    // The file changed externally while the write was unknown, so the
    // reconcile read mismatches and the dialog opens with the token still
    // unknown.
    disk = { text: "external edit", token: "t500" };
    await panel.save();
    check(
      "an unknown-token mismatch opens the conflict dialog before the Overwrite",
      puts.length === 1 && conflictOverlay(panel) !== null,
    );
    // Typed after the mismatched save, so the Overwrite sends different
    // text than the timed-out save did and reconciliation must match the
    // Overwrite's.
    stub.type("newer");
    overwriteButton(panel).click();
    await flush();
    await panel.save();
    check(
      "an Overwrite whose timed-out write landed lets the next save adopt the fresh token",
      puts.length === 3 && puts[2].expectedToken === "t600" && puts[2].text === "newer",
    );
    check(
      "the adopted-token save after an Overwrite does not reopen the conflict dialog",
      conflictOverlay(panel) === null,
    );
    check("the adopted-token save after an Overwrite clears the dirty state", !panel.isDirty());
    panel.dispose();
  }
});

if (failures.length > 0) {
  console.error(`editor-save-timeout: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-save-timeout: all assertions passed");
process.exit(0);
