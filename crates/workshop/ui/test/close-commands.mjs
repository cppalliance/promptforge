// Integration test for the confirm-then-close path: WorkshopPart's default
// confirmClose (@workshop/platform/workshop-part.ts), the editor's
// unsaved-changes confirmation (src/parts/editor/editor-panel.ts), and the
// Close and Close Others commands (src/parts/editor/editor-commands.ts,
// registered by editor.contribution.ts). Mounts a real Dockview dock in
// jsdom with synthetic panel types - "doc" builds a real EditorPanel over a
// stub surface and stub file I/O, "probe" is a bare WorkshopPart, "plain"
// is a renderer that is not a WorkshopPart, and "pinned" registers
// closable: false - and runs every command through the shared registries,
// so the contribution's run bodies must forward their { panelId } argument.
// Covers: Close targets the { panelId } panel over the active one, else the
// active panel; a non-closable panel stays; a plain renderer and a clean
// WorkshopPart close without a prompt; an unsaved editor is activated and
// prompts, and Cancel, Escape, or a failed save keep it open while the
// command settles; a second Close while the prompt is up declines; Close
// Others activates and prompts each unsaved editor in turn, closes nothing
// when either is cancelled (a save already made stands), and closes every
// other closable panel once both are answered, sparing the non-closable
// one; an editor edited after its confirmation voids the batch; a panel
// that leaves the dock mid-batch is skipped without disturbing the group;
// an editor that leaves the dock with its prompt up still settles Close
// and Close Others, and the batch closes nothing; Ctrl+F4 still closes the
// active editor only while it has text focus.
// Run: node --test test/close-commands.mjs
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
      import "./src/parts/editor/editor.contribution.ts";
      export { createDockview, themeDark } from "dockview";
      export {
        registerPanelFactory,
        registerPanelType,
        resolvePanelContent,
      } from "@workshop/platform/panel-registry";
      export { WorkshopPart } from "@workshop/platform/workshop-part";
      export { Commands } from "@workshop/platform/command-registry";
      export { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
      export { detectPlatform } from "@workshop/platform/keybinding-parser";
      export { getService } from "@workshop/platform/service-registry";
      export { initZones, openInZone, resetZones } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/parts/layout/panel-types.ts";
      export { KeybindingDispatcher } from "./src/parts/layout/keybinding-dispatcher.ts";
      export { EditorPanel } from "./src/parts/editor/editor-panel.ts";
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
  // The modules under test import colocated CSS; strip it - jsdom applies
  // no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
});
const { window } = dom;

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
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

globalThis.fetch = async (url) => {
  throw new Error(`unexpected fetch in the close-commands test: ${url}`);
};

for (const key of [
  "document",
  "navigator",
  "location",
  "Window",
  "HTMLElement",
  "HTMLInputElement",
  "HTMLTextAreaElement",
  "HTMLButtonElement",
  "Node",
  "Element",
  "KeyboardEvent",
  "MutationObserver",
  "ResizeObserver",
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

const bundlePath = path.join(os.tmpdir(), "promptforge-close-commands-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  registerPanelFactory,
  registerPanelType,
  resolvePanelContent,
  WorkshopPart,
  Commands,
  CONTEXT_KEY_SERVICE,
  detectPlatform,
  getService,
  initZones,
  openInZone,
  resetZones,
  createPanelComponent,
  createPanelTabComponent,
  KeybindingDispatcher,
  EditorPanel,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// A command's promise, watched without awaiting it: a close waiting on a
// prompt must not hang the test, and a rejection must not crash it.
const rejections = [];
function track(promise) {
  const state = { settled: false };
  promise.then(
    () => {
      state.settled = true;
    },
    (error) => {
      state.settled = true;
      rejections.push(String(error?.stack ?? error));
    },
  );
  return state;
}

// The EditorSurface contract over plain fields. Each surface registers
// under its document's path when the panel opens it, so a test can type
// into a panel it only knows by name.
const surfaces = new Map();
function createStubSurface() {
  const listeners = new Set();
  return {
    element: window.document.createElement("div"),
    currentText: "",
    dirty: false,
    open(document) {
      surfaces.set(document.path, this);
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
    editorView() {
      return null;
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

// File I/O stubs: every read answers an empty file; writes are recorded,
// and fail while failWrites is set.
const writes = [];
let failWrites = false;
const readDoc = async (filePath) => ({ path: filePath, size: 0, token: "t1", text: "" });
const writeDoc = async (filePath, text) => {
  if (failWrites) {
    throw new Error("the disk is full");
  }
  writes.push({ path: filePath, text });
  return { path: filePath, size: text.length, token: "t2", text };
};

class ProbePart extends WorkshopPart {
  create() {}
}

// One lazy "feature directory" serving every synthetic type, all in the
// main zone so they share one group.
const feature = {
  register() {
    registerPanelFactory(
      "doc",
      () => new EditorPanel({ createSurface: createStubSurface, readFile: readDoc, writeFile: writeDoc }),
    );
    registerPanelFactory("probe", () => new ProbePart());
    registerPanelFactory("plain", () => ({ element: window.document.createElement("div"), init() {} }));
    registerPanelFactory("pinned", () => new ProbePart());
  },
};
const load = () => Promise.resolve(feature);
registerPanelType({
  type: "doc",
  title: (params) => String(params.path),
  defaultZone: "main",
  panelId: (params) => `doc:${params.path}`,
  load,
});
registerPanelType({ type: "probe", title: "Probe", defaultZone: "main", load });
registerPanelType({ type: "plain", title: "Plain", defaultZone: "main", load });
registerPanelType({ type: "pinned", title: "Pinned", defaultZone: "main", closable: false, load });

let dock = null;
function freshDock() {
  const element = window.document.createElement("div");
  element.className = "ws-dock";
  window.document.body.appendChild(element);
  dock = createDockview(element, {
    createComponent: createPanelComponent,
    createTabComponent: createPanelTabComponent,
    theme: themeDark,
    disableFloatingGroups: true,
    hideBorders: true,
    locked: false,
    noPanelsOverlay: "emptyGroup",
  });
  initZones(dock);
  resetZones();
}

async function openDoc(name) {
  const panel = openInZone("doc", { path: name });
  await flush();
  return panel;
}

async function openPanel(type) {
  const panel = openInZone(type, {});
  await flush();
  return panel;
}

const CLOSE = "workbench.action.closeActiveEditor";
const CLOSE_OTHERS = "workbench.action.closeOtherEditors";

const isOpen = (panel) => dock.getPanel(panel.id) === panel;
const partOf = (panel) => resolvePanelContent(panel.view.content);
const isDirty = (panel) => partOf(panel) instanceof EditorPanel && partOf(panel).isDirty();
const promptsIn = (panel) => partOf(panel).element.querySelectorAll(".ws-editor-close-overlay").length;

/** Clicks one button of the panel's unsaved-changes prompt; false when none is up. */
function clickPrompt(panel, label) {
  const button = [...partOf(panel).element.querySelectorAll(".ws-editor-close__button")].find(
    (candidate) => candidate.textContent === label,
  );
  button?.click();
  return button !== undefined;
}

// --- Close: the { panelId } target, else the active panel ---------------------

freshDock();
const first = await openDoc("first.txt");
const second = await openDoc("second.txt");
await Commands.execute(CLOSE, { panelId: first.id });
await flush();
check("Close with { panelId } closes that panel", !isOpen(first));
check("Close with { panelId } leaves the active panel open", isOpen(second));
await Commands.execute(CLOSE);
await flush();
check("Close with no argument closes the active panel", !isOpen(second));

const pinned = await openPanel("pinned");
await Commands.execute(CLOSE, { panelId: pinned.id });
await Commands.execute(CLOSE);
await flush();
check("Close leaves a closable: false panel open, by id or as the active panel", isOpen(pinned));

const plain = await openPanel("plain");
const probe = await openPanel("probe");
await Commands.execute(CLOSE, { panelId: plain.id });
await Commands.execute(CLOSE, { panelId: probe.id });
await flush();
check("Close closes a panel that is not a WorkshopPart directly", !isOpen(plain));
check("Close closes a clean WorkshopPart through its default confirmClose", !isOpen(probe));

// --- Close on an unsaved editor: activate, prompt, and honor the answer -------

const unsaved = await openDoc("unsaved.txt");
const other = await openDoc("other.txt");
surfaces.get("unsaved.txt").type("draft\n");

const cancelled = track(Commands.execute(CLOSE, { panelId: unsaved.id }));
await flush();
check(
  "Close on an unsaved inactive editor activates it and prompts instead of closing",
  dock.activePanel === unsaved && isOpen(unsaved) && promptsIn(unsaved) === 1,
);
check("the prompt offers Cancel", clickPrompt(unsaved, "Cancel"));
await flush();
check("Cancel keeps the unsaved editor open and dirty", isOpen(unsaved) && isDirty(unsaved));
check("Close settles after Cancel", cancelled.settled);

const escaped = track(Commands.execute(CLOSE, { panelId: unsaved.id }));
await flush();
window.document.dispatchEvent(
  new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
);
await flush();
check("Escape dismisses the prompt and keeps the unsaved editor open", promptsIn(unsaved) === 0 && isOpen(unsaved));
check("Close settles after Escape", escaped.settled);

failWrites = true;
const failedSave = track(Commands.execute(CLOSE, { panelId: unsaved.id }));
await flush();
check("the prompt offers Save", clickPrompt(unsaved, "Save"));
await flush();
failWrites = false;
check("a failed save in the prompt keeps the unsaved editor open", isOpen(unsaved) && isDirty(unsaved));
check("Close settles after a failed save", failedSave.settled);

const asking = track(Commands.execute(CLOSE, { panelId: unsaved.id }));
await flush();
const again = track(Commands.execute(CLOSE, { panelId: unsaved.id }));
await flush();
check(
  "a second Close while the prompt is up declines at once and opens no second prompt",
  again.settled && !asking.settled && promptsIn(unsaved) === 1 && isOpen(unsaved),
);
check("the prompt offers Discard", clickPrompt(unsaved, "Discard"));
await flush();
check("Discard closes the unsaved editor", !isOpen(unsaved) && asking.settled);
check("closing the unsaved editor leaves the other editor open", isOpen(other));

// --- Close Others: confirm the batch in turn, then close it together ----------

freshDock();
const keep = await openDoc("keep.txt");
const one = await openDoc("one.txt");
const clean = await openDoc("clean.txt");
const bare = await openPanel("probe");
const raw = await openPanel("plain");
const fixed = await openPanel("pinned");
const two = await openDoc("two.txt");
const others = [one, clean, bare, raw, two];
check(
  "the Close Others panels share one group",
  [one, clean, bare, raw, fixed, two].every((panel) => panel.group === keep.group),
);
surfaces.get("one.txt").type("one\n");
surfaces.get("two.txt").type("two\n");

keep.api.setActive();
const cancelFirst = track(Commands.execute(CLOSE_OTHERS, { panelId: keep.id }));
await flush();
check(
  "Close Others activates the first unsaved editor and prompts for it",
  dock.activePanel === one && promptsIn(one) === 1,
);
check("the second unsaved editor waits its turn", promptsIn(two) === 0);
clickPrompt(one, "Cancel");
await flush();
check(
  "cancelling the first prompt closes nothing and never prompts the second",
  [keep, ...others, fixed].every(isOpen) && promptsIn(two) === 0,
);
check("Close Others settles after the first Cancel", cancelFirst.settled);

const writesBefore = writes.length;
const cancelSecond = track(Commands.execute(CLOSE_OTHERS, { panelId: keep.id }));
await flush();
clickPrompt(one, "Save");
await flush();
check(
  "answering the first prompt activates and prompts the second unsaved editor",
  dock.activePanel === two && promptsIn(two) === 1,
);
clickPrompt(two, "Cancel");
await flush();
check("cancelling the second prompt closes nothing", [keep, ...others, fixed].every(isOpen));
check(
  "a save made before the Cancel stands",
  writes.length === writesBefore + 1 && writes.at(-1)?.text === "one\n" && !isDirty(one),
);
check("Close Others settles after the second Cancel", cancelSecond.settled);

surfaces.get("one.txt").type("one again\n");
const answered = track(Commands.execute(CLOSE_OTHERS, { panelId: keep.id }));
await flush();
clickPrompt(one, "Discard");
await flush();
clickPrompt(two, "Discard");
await flush();
check(
  "answering both prompts closes every other closable panel, clean ones included",
  others.every((panel) => !isOpen(panel)),
);
check("Close Others spares its target and the non-closable panel", isOpen(keep) && isOpen(fixed));
check("Close Others settles once the batch closes", answered.settled);

// An editor confirmed clean, then edited while a later prompt is up, never
// answered for its new changes: the batch closes nothing.
const edited = await openDoc("edited.txt");
const three = await openDoc("three.txt");
surfaces.get("three.txt").type("three\n");
const voided = track(Commands.execute(CLOSE_OTHERS, { panelId: keep.id }));
await flush();
check("the batch reaches the unsaved editor's prompt", promptsIn(three) === 1);
surfaces.get("edited.txt").type("typed during the prompt\n");
clickPrompt(three, "Discard");
await flush();
check(
  "an editor edited after its confirmation voids the batch",
  isOpen(edited) && isOpen(three) && isOpen(keep),
);
check("Close Others settles when the batch is voided", voided.settled);

// A panel that leaves the dock mid-batch, the way Dockview's own tab close
// removes one, is skipped rather than closed a second time.
for (const panel of [edited, three]) {
  if (isOpen(panel)) dock.removePanel(panel);
}
const gone = await openDoc("gone.txt");
const four = await openDoc("four.txt");
surfaces.get("four.txt").type("four\n");
const skipping = track(Commands.execute(CLOSE_OTHERS, { panelId: keep.id }));
await flush();
check("the batch reaches the last unsaved editor's prompt", promptsIn(four) === 1);
dock.removePanel(gone);
clickPrompt(four, "Discard");
await flush();
check(
  "a panel that left the dock mid-batch is skipped while the rest close",
  !isOpen(four) && isOpen(keep) && isOpen(fixed),
);
check(
  "closing the batch leaves the group's panel list intact",
  keep.group.panels.length === 2 && keep.group.panels.includes(keep) && keep.group.panels.includes(fixed),
);
check("Close Others settles after skipping the departed panel", skipping.settled);

// An editor that leaves the dock with its prompt up, the way Dockview's own
// tab close removes one, answers through neither a button nor Escape: the
// command awaiting it still settles, and the batch closes nothing.
const bystander = await openDoc("bystander.txt");
const five = await openDoc("five.txt");
surfaces.get("five.txt").type("five\n");
const departedBatch = track(Commands.execute(CLOSE_OTHERS, { panelId: keep.id }));
await flush();
check("the batch reaches the prompt of the editor about to leave", promptsIn(five) === 1);
dock.removePanel(five);
await flush();
check("Close Others settles when its prompting editor leaves the dock", departedBatch.settled);
check(
  "a batch whose prompting editor left closes nothing",
  isOpen(bystander) && isOpen(keep) && isOpen(fixed),
);

const six = await openDoc("six.txt");
surfaces.get("six.txt").type("six\n");
const departedClose = track(Commands.execute(CLOSE, { panelId: six.id }));
await flush();
check("Close reaches the prompt of the editor about to leave", promptsIn(six) === 1);
dock.removePanel(six);
await flush();
check("Close settles when its prompting editor leaves the dock", departedClose.settled);

// --- Ctrl+F4 through the keybinding dispatcher ---------------------------------

const statusErrors = [];
const dispatcher = new KeybindingDispatcher({
  status: { show() {}, showError: (message) => statusErrors.push(message), clear() {} },
});
const contextKeys = getService(CONTEXT_KEY_SERVICE);
const activeEditorKey = contextKeys.createKey("activeEditor", undefined);
const textFocusKey = contextKeys.createKey("editorTextFocus", false);
// ctrlcmd resolves against the host platform the shared registry detected.
const chordModifier = detectPlatform() === "mac" ? { metaKey: true } : { ctrlKey: true };
function pressCloseChord() {
  window.document.body.dispatchEvent(
    new window.KeyboardEvent("keydown", { key: "F4", code: "F4", bubbles: true, cancelable: true, ...chordModifier }),
  );
}

const focused = await openDoc("focused.txt");
activeEditorKey.set(focused.id);
textFocusKey.set(false);
pressCloseChord();
await flush();
check("Ctrl+F4 leaves the active editor open without editor text focus", isOpen(focused));
textFocusKey.set(true);
pressCloseChord();
await flush();
check("Ctrl+F4 closes the active editor while it has text focus", !isOpen(focused));
check("no close command rejected through the dispatcher", statusErrors.length === 0);
dispatcher.dispose();
activeEditorKey.reset();
textFocusKey.reset();

check(`no close command rejected (got: ${rejections.join("; ")})`, rejections.length === 0);

if (failures.length > 0) {
  console.error(`close-commands: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("close-commands: all assertions passed");
process.exit(0);
