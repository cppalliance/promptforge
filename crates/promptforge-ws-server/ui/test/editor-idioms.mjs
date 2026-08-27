// Editor idiom test (step 16, src/ui/workshop/editor-surface.ts): the
// readOnly toggle runs through a Compartment - one reconfigure dispatch,
// so document text, dirty tracking, and the live view all survive a
// toggle - and reloads into a live view dispatch a transaction tagged
// with the externalUpdate annotation, distinguishable from local typing
// via the exported isExternalUpdate helper. Runs CodeMirror under jsdom
// with the same measurement shims as editor-panel.mjs.
// Run: node test/editor-idioms.mjs
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
      export {
        CodeMirrorSurface,
        externalUpdate,
        isExternalUpdate,
      } from "./src/ui/workshop/editor-surface.ts";
      export { redo, undo, undoDepth } from "@codemirror/commands";
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

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7911/",
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

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-idioms-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, CodeMirrorSurface, externalUpdate, isExternalUpdate, redo, undo, undoDepth } =
  await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const FILE_PATH = "C:\\project\\notes.md";

function contentEditableOf(surface) {
  return surface.element.querySelector(".cm-content")?.getAttribute("contenteditable");
}

await assertNoLeaks(lifecycle, async () => {
  // --- readOnly toggles through the compartment ----------------------------

  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: FILE_PATH, text: "alpha\n" });

  check("a fresh surface accepts input", contentEditableOf(surface) === "true");
  check("a fresh surface state is not readOnly", surface.view.state.readOnly === false);

  surface.setReadOnly(true);
  check("setReadOnly(true) flips the state's readOnly facet", surface.view.state.readOnly === true);
  check("setReadOnly(true) makes the content non-editable", contentEditableOf(surface) === "false");

  surface.setReadOnly(false);
  check(
    "setReadOnly(false) restores the editable state",
    surface.view.state.readOnly === false && contentEditableOf(surface) === "true",
  );

  // --- toggling reconfigures in place: no state rebuild ---------------------

  // The view is TypeScript-private but reachable at runtime, same as in
  // editor-panel.mjs: the assertions drive the real dispatch path.
  surface.view.dispatch({ changes: { from: 0, insert: "x" } });
  check("an edit before the toggle dirties the surface", surface.isDirty());

  const viewBefore = surface.view;
  const editorNodeBefore = surface.element.querySelector(".cm-editor");
  surface.setReadOnly(true);
  surface.setReadOnly(false);
  check(
    "toggling keeps the same view and editor DOM node - no rebuild",
    surface.view === viewBefore &&
      surface.element.querySelector(".cm-editor") === editorNodeBefore,
  );
  check("toggling keeps the document text", surface.text() === "xalpha\n");
  check("toggling keeps the dirty state", surface.isDirty());

  // The unrelated dirty-tracking extension survives the toggles: reverting
  // the edit still lands on the updateListener and clears dirty.
  surface.view.dispatch({ changes: { from: 0, to: 1 } });
  check("the dirty updateListener still runs after toggles", !surface.isDirty());

  // --- externalUpdate tags server-originated reloads ------------------------

  // Capture the transactions the surface dispatches by intercepting the
  // view's update entry point; open() keeps the view alive on reload, so
  // the interception survives it.
  const captured = [];
  const view = surface.view;
  const realUpdate = view.update.bind(view);
  view.update = (transactions) => {
    captured.push(...transactions);
    realUpdate(transactions);
  };

  surface.open({ path: FILE_PATH, text: "from the server\n" });
  check(
    "a reload into a live view dispatches a transaction tagged externalUpdate",
    captured.some((tr) => isExternalUpdate(tr)),
  );
  check(
    "the annotation itself reads back off the transaction",
    captured.some((tr) => tr.annotation(externalUpdate) === true),
  );
  check("the reload replaced the document text", surface.text() === "from the server\n");
  check("a reload is not dirty", !surface.isDirty());

  // The reload deliberately enters undo history (plan step 16, decision 3):
  // undo can cross it back into pre-reload text, and dirty tracking flags
  // the result against the reloaded baseline.
  check("the reload keeps undo history", undoDepth(view.state) > 0);
  undo(view);
  check("undo crosses the reload into the pre-reload text", surface.text() === "alpha\n");
  check("undoing past the reload marks the surface dirty", surface.isDirty());
  redo(view);
  check(
    "redo reapplies the reload and returns to clean",
    surface.text() === "from the server\n" && !surface.isDirty(),
  );

  const capturedBefore = captured.length;
  view.dispatch({ changes: { from: 0, insert: "typed " } });
  check(
    "a typed change is not tagged externalUpdate",
    captured.length === capturedBefore + 1 && !isExternalUpdate(captured.at(-1)),
  );
  check("the typed change landed", surface.text() === "typed from the server\n");

  surface.dispose();
  check("dispose still tears the editor down", !surface.element.querySelector(".cm-editor"));

  // --- readOnly set before open() seeds the first state ---------------------

  const lockedSurface = new CodeMirrorSurface();
  window.document.body.appendChild(lockedSurface.element);
  lockedSurface.setReadOnly(true);
  lockedSurface.open({ path: "C:\\project\\b.txt", text: "locked\n" });
  check(
    "readOnly set before open applies to the first state",
    lockedSurface.view.state.readOnly === true && contentEditableOf(lockedSurface) === "false",
  );
  lockedSurface.dispose();
});

if (failures.length > 0) {
  console.error(`editor-idioms: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-idioms: all assertions passed");
process.exit(0);
