// Save As test for the editor panel (src/parts/editor/editor-panel.ts):
// the OS save dialog's replace confirmation is the user's consent, so a
// Save As onto an existing file retries once with the target's token
// instead of opening the conflict dialog, which acts on the old path.
// Covers six cases: Save As onto an existing file writes the target,
// retargets the panel, and leaves the old file untouched; a target read
// failure shows an error naming the target and doesn't retarget; a retry
// that conflicts again does the same; a 408 shows the save timeout
// message, doesn't retarget, and leaves the open file's token known; a
// 408 on the retry onto an existing target does the same; and a retry
// after a 408 whose write landed takes the 409 path and converges.
// Drives the real EditorPanel with a stubbed surface and an in-memory
// disk, the same way editor-save-timeout.mjs does.
// Run: node test/editor-save-as.mjs
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

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-save-as-test.mjs");
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

const OLD_PATH = "C:\\project\\a.txt";
const TARGET_PATH = "C:\\elsewhere\\b.txt";

// The panel's contract stub, mirroring editor-save-timeout.mjs: markSaved
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

// The typed failures the reader and writer surface, matching the
// boundary's codes so the panel's narrowing helpers recognize them.
const deadlineError = () =>
  new CatalogError(ErrorCatalog.DeadlineElapsed, "save timed out", { status: 408 });
const conflictError = () =>
  new CatalogError(ErrorCatalog.ModifiedConflict, "file changed on disk", { status: 409 });
const notTextError = () =>
  new CatalogError(ErrorCatalog.HttpStatus, "the file is not UTF-8 text", { status: 422 });
const notFoundError = () =>
  new CatalogError(ErrorCatalog.HttpStatus, "no such file", { status: 404 });

// An in-memory workspace with the server's token rule: a write whose
// expected token doesn't match the file on disk is refused with a 409,
// and a null token matches only a file that doesn't exist yet. `script`
// scripts the next writes in order: "timeout" answers 408 before writing,
// "timeout-landed" writes and then answers 408, and null follows the
// token rule. `unreadable` maps a path to the error its read throws, and
// `afterRead` runs after each successful read.
function createDisk(files) {
  const entries = new Map(Object.entries(files));
  let serial = 0;
  const disk = {
    entries,
    puts: [],
    reads: [],
    script: [],
    unreadable: new Map(),
    afterRead: () => {},
    readFile: async (filePath) => {
      disk.reads.push(filePath);
      const failure = disk.unreadable.get(filePath);
      if (failure !== undefined) throw failure();
      const file = entries.get(filePath);
      if (file === undefined) throw notFoundError();
      disk.afterRead(filePath);
      return { path: filePath, size: file.text.length, token: file.token, text: file.text };
    },
    writeFile: async (filePath, text, expectedToken) => {
      disk.puts.push({ path: filePath, text, expectedToken });
      const scripted = disk.script.shift() ?? null;
      if (scripted === "timeout") throw deadlineError();
      if ((entries.get(filePath)?.token ?? null) !== expectedToken) throw conflictError();
      serial += 1;
      const file = { text, token: `t${serial}` };
      entries.set(filePath, file);
      if (scripted === "timeout-landed") throw deadlineError();
      return { path: filePath, size: text.length, token: file.token, text };
    },
  };
  return disk;
}

function createPanel(stub, disk) {
  return new EditorPanel({
    createSurface: () => stub,
    readFile: disk.readFile,
    writeFile: disk.writeFile,
  });
}

const errorBar = (panel) => panel.element.querySelector(".ws-editor-panel__error");
const conflictOverlay = (panel) => panel.element.querySelector(".ws-editor-conflict-overlay");

await assertNoLeaks(lifecycle, async () => {
  // --- Save As onto an existing file replaces it and retargets ---------------
  {
    const stub = createStubSurface();
    const disk = createDisk({
      [OLD_PATH]: { text: "old", token: "t-old" },
      [TARGET_PATH]: { text: "target", token: "t-target" },
    });
    const panel = createPanel(stub, disk);
    panel.init(fakeParameters(OLD_PATH));
    await flush();

    stub.type("new");
    await panel.saveAs(TARGET_PATH);
    check(
      "Save As onto an existing file retries once with the target's token",
      disk.puts.length === 2 &&
        disk.puts[0].expectedToken === null &&
        disk.puts[1].expectedToken === "t-target",
    );
    check(
      "Save As onto an existing file writes the live text to the target",
      disk.entries.get(TARGET_PATH)?.text === "new",
    );
    check("Save As onto an existing file retargets the panel", panel.filePath() === TARGET_PATH);
    check("Save As onto an existing file leaves the panel clean", !panel.isDirty());
    check(
      "Save As onto an existing file leaves the old file untouched",
      disk.puts.every((put) => put.path === TARGET_PATH) &&
        disk.entries.get(OLD_PATH)?.text === "old" &&
        disk.entries.get(OLD_PATH)?.token === "t-old",
    );
    check(
      "Save As onto an existing file never opens the conflict dialog",
      conflictOverlay(panel) === null && errorBar(panel) === null,
    );

    // The panel adopted the target's fresh token, so the next save lands.
    stub.type("newer");
    await panel.save();
    check(
      "a save after Save As onto an existing file writes the target with its fresh token",
      disk.entries.get(TARGET_PATH)?.text === "newer" &&
        conflictOverlay(panel) === null &&
        errorBar(panel) === null,
    );
    panel.dispose();
  }

  // --- A target read failure shows an error and doesn't retarget -------------
  {
    const stub = createStubSurface();
    const disk = createDisk({
      [OLD_PATH]: { text: "old", token: "t-old" },
      [TARGET_PATH]: { text: "binary", token: "t-target" },
    });
    disk.unreadable.set(TARGET_PATH, notTextError);
    const panel = createPanel(stub, disk);
    panel.init(fakeParameters(OLD_PATH));
    await flush();

    stub.type("new");
    await panel.saveAs(TARGET_PATH);
    check(
      "an unreadable Save As target shows an error naming the target",
      errorBar(panel)?.textContent.includes(TARGET_PATH),
    );
    check("an unreadable Save As target doesn't retarget", panel.filePath() === OLD_PATH);
    check("an unreadable Save As target leaves the panel dirty", panel.isDirty());
    check("an unreadable Save As target never opens the conflict dialog", conflictOverlay(panel) === null);
    check(
      "an unreadable Save As target is never written",
      disk.puts.length === 1 && disk.entries.get(TARGET_PATH)?.text === "binary",
    );
    panel.dispose();
  }

  // --- A retry that conflicts again shows an error and doesn't retarget ------
  {
    const stub = createStubSurface();
    const disk = createDisk({
      [OLD_PATH]: { text: "old", token: "t-old" },
      [TARGET_PATH]: { text: "target", token: "t-target" },
    });
    // An outside edit lands between the target read and the retry.
    disk.afterRead = (filePath) => {
      if (filePath === TARGET_PATH) {
        disk.entries.set(TARGET_PATH, { text: "outside", token: "t-outside" });
      }
    };
    const panel = createPanel(stub, disk);
    panel.init(fakeParameters(OLD_PATH));
    await flush();

    stub.type("new");
    await panel.saveAs(TARGET_PATH);
    check(
      "a Save As retry that conflicts again shows an error naming the target",
      errorBar(panel)?.textContent.includes(TARGET_PATH),
    );
    check("a Save As retry that conflicts again doesn't retarget", panel.filePath() === OLD_PATH);
    check(
      "a Save As retry that conflicts again never opens the conflict dialog",
      conflictOverlay(panel) === null,
    );
    check(
      "a Save As retry that conflicts again retries only once and keeps the outside edit",
      disk.puts.length === 2 && disk.entries.get(TARGET_PATH)?.text === "outside",
    );
    panel.dispose();
  }

  // --- A 408 shows the timeout message and doesn't retarget ------------------
  {
    const stub = createStubSurface();
    const disk = createDisk({ [OLD_PATH]: { text: "old", token: "t-old" } });
    disk.script = ["timeout"];
    const panel = createPanel(stub, disk);
    panel.init(fakeParameters(OLD_PATH));
    await flush();

    stub.type("new");
    await panel.saveAs(TARGET_PATH);
    check(
      "a Save As 408 shows the save timeout message",
      errorBar(panel)?.textContent.includes("may or may not"),
    );
    check("a Save As 408 doesn't retarget", panel.filePath() === OLD_PATH);
    check("a Save As 408 leaves the panel dirty", panel.isDirty());
    check("a Save As 408 never opens the conflict dialog", conflictOverlay(panel) === null);

    // The timeout was the target's, not the open file's: the next save
    // writes the open file with its known token, without a reconcile read.
    const readsBefore = disk.reads.length;
    await panel.save();
    check(
      "a Save As 408 leaves the open file's token known",
      disk.reads.length === readsBefore &&
        disk.puts.at(-1)?.path === OLD_PATH &&
        disk.puts.at(-1)?.expectedToken === "t-old" &&
        disk.entries.get(OLD_PATH)?.text === "new",
    );
    panel.dispose();
  }

  // --- A 408 on the retry shows the timeout and doesn't retarget -------------
  {
    const stub = createStubSurface();
    const disk = createDisk({
      [OLD_PATH]: { text: "old", token: "t-old" },
      [TARGET_PATH]: { text: "target", token: "t-target" },
    });
    // The null-token write conflicts; the retry with the target's token
    // times out before writing.
    disk.script = [null, "timeout"];
    const panel = createPanel(stub, disk);
    panel.init(fakeParameters(OLD_PATH));
    await flush();

    stub.type("new");
    await panel.saveAs(TARGET_PATH);
    check(
      "a Save As retry 408 comes from the retry with the target's token",
      disk.puts.length === 2 && disk.puts[1].expectedToken === "t-target",
    );
    check(
      "a Save As retry 408 shows the save timeout message",
      errorBar(panel)?.textContent.includes("may or may not"),
    );
    check("a Save As retry 408 doesn't retarget", panel.filePath() === OLD_PATH);
    check("a Save As retry 408 leaves the panel dirty", panel.isDirty());
    check("a Save As retry 408 never opens the conflict dialog", conflictOverlay(panel) === null);
    check(
      "a Save As retry 408 leaves the target untouched",
      disk.entries.get(TARGET_PATH)?.text === "target",
    );

    const readsBefore = disk.reads.length;
    await panel.save();
    check(
      "a Save As retry 408 leaves the open file's token known",
      disk.reads.length === readsBefore &&
        disk.puts.at(-1)?.path === OLD_PATH &&
        disk.puts.at(-1)?.expectedToken === "t-old" &&
        disk.entries.get(OLD_PATH)?.text === "new",
    );
    panel.dispose();
  }

  // --- A retry after a landed 408 takes the 409 path and converges -----------
  {
    const stub = createStubSurface();
    const disk = createDisk({ [OLD_PATH]: { text: "old", token: "t-old" } });
    disk.script = ["timeout-landed"];
    const panel = createPanel(stub, disk);
    panel.init(fakeParameters(OLD_PATH));
    await flush();

    stub.type("new");
    await panel.saveAs(TARGET_PATH);
    check("a landed Save As 408 still doesn't retarget", panel.filePath() === OLD_PATH);
    const landedToken = disk.entries.get(TARGET_PATH)?.token;

    errorBar(panel)?.remove();
    await panel.saveAs(TARGET_PATH);
    check(
      "a Save As retry after a landed 408 retries once with the landed token",
      disk.puts.length === 3 &&
        disk.puts[1].expectedToken === null &&
        disk.puts[2].expectedToken === landedToken,
    );
    check("a Save As retry after a landed 408 retargets the panel", panel.filePath() === TARGET_PATH);
    check("a Save As retry after a landed 408 leaves the panel clean", !panel.isDirty());
    check(
      "a Save As retry after a landed 408 shows no error and no conflict dialog",
      errorBar(panel) === null && conflictOverlay(panel) === null,
    );
    panel.dispose();
  }
});

if (failures.length > 0) {
  console.error(`editor-save-as: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-save-as: all assertions passed");
process.exit(0);
