// Unit test for the text-control service
// (src/services/text-control-service.ts): per-widget undo/redo/select-all
// adapters registered on DOM roots, focus tracking across the document,
// the inputFocus / editorTextFocus / textInputFocus context keys, and the
// execCommand fallback for native editables (including the remembered
// target, since a menu click steals focus before the command runs).
// Also covers the edit contribution (src/ui/menu/edit.contribution.ts):
// the six edit rows register with the textInputFocus precondition and
// their commands route through the shared service singleton - adapter
// when one is active, the native path otherwise.
// Bundles the modules with esbuild and drives them against jsdom.
// Run: node --test test/text-control-service.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { TextControlService, TEXT_CONTROL_SERVICE } from "./src/services/text-control-service.ts";
      export { ContextKeyService } from "./src/services/context-key-service.ts";
      export { getService } from "./src/services/service-registry.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus, MenuId } from "./src/services/menu-registry.ts";
      import "./src/ui/menu/edit.contribution.ts";
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

const dom = new JSDOM("<!DOCTYPE html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.Element = window.Element;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLInputElement = window.HTMLInputElement;
globalThis.HTMLTextAreaElement = window.HTMLTextAreaElement;
globalThis.Node = window.Node;

const { TextControlService, TEXT_CONTROL_SERVICE, ContextKeyService, getService, Commands, Menus, MenuId } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Self-registration ---------------------------------------------------------

check(
  "the TEXT_CONTROL_SERVICE token self-registers",
  getService(TEXT_CONTROL_SERVICE) instanceof TextControlService,
);

// --- Fixture ---------------------------------------------------------------------

const execCalls = [];
window.document.execCommand = (command) => {
  execCalls.push(command);
  return true;
};

const contextKeys = new ContextKeyService();
const service = new TextControlService(contextKeys, window.document);
const body = window.document.body;

function addElement(tag) {
  const element = window.document.createElement(tag);
  body.appendChild(element);
  return element;
}

const input = addElement("input"); // type text by default
const textarea = addElement("textarea");
const checkbox = addElement("input");
checkbox.type = "checkbox";
const editable = addElement("div");
editable.setAttribute("contenteditable", "");
editable.tabIndex = 0;
const button = addElement("button");

const keys = () => ({
  input: contextKeys.getValue("inputFocus"),
  editor: contextKeys.getValue("editorTextFocus"),
  text: contextKeys.getValue("textInputFocus"),
});
const allClear = () => keys().input === false && keys().editor === false && keys().text === false;

// --- Focus keys over native editables ----------------------------------------------

check("the focus keys start false", allClear());

input.focus();
check("a focused text input sets inputFocus and textInputFocus", keys().input === true && keys().text === true);
check("a focused text input leaves editorTextFocus false", keys().editor === false);

textarea.focus();
check("a focused textarea sets inputFocus and textInputFocus", keys().input === true && keys().text === true);

checkbox.focus();
check("a non-text input clears inputFocus", keys().input === false);
check("a non-text input clears textInputFocus", keys().text === false);

editable.focus();
check("a focused contentEditable sets textInputFocus", keys().text === true);
check("a focused contentEditable leaves inputFocus false", keys().input === false);

button.focus();
check("focusing a non-editable clears every focus key", allClear());

input.focus();
input.blur();
check("blurring to nothing clears every focus key", allClear());

// --- Adapter registration and active tracking -----------------------------------------

const host = addElement("div");
const inner = window.document.createElement("div");
inner.tabIndex = 0;
host.appendChild(inner);

const calls = { undo: 0, redo: 0, selectAll: 0 };
let undoDepth = 1;
let redoDepth = 1;
const control = {
  kind: "codemirror",
  undo: () => {
    calls.undo += 1;
  },
  redo: () => {
    calls.redo += 1;
  },
  selectAll: () => {
    calls.selectAll += 1;
  },
  canUndo: () => undoDepth > 0,
  canRedo: () => redoDepth > 0,
};

inner.focus();
check("no adapter is active before registration", service.active === null);

const registration = service.register(host, control);
check("the focused subtree's adapter is active", service.active === control);
check("a codemirror adapter sets editorTextFocus", keys().editor === true);
check("a codemirror adapter sets textInputFocus", keys().text === true);
check("a codemirror adapter leaves inputFocus false", keys().input === false);

button.focus();
check("focus outside every root leaves no active adapter", service.active === null);
check("losing adapter focus clears editorTextFocus", keys().editor === false);

inner.focus();
check("returning focus reactivates the adapter", service.active === control);

// --- Command routing --------------------------------------------------------------------

const execBefore = execCalls.length;
service.undo();
check("undo routes to the active adapter when it can undo", calls.undo === 1);
check("adapter undo does not touch execCommand", execCalls.length === execBefore);

service.redo();
check("redo routes to the active adapter when it can redo", calls.redo === 1);

service.selectAll();
check("selectAll routes to the active adapter", calls.selectAll === 1);

undoDepth = 0;
service.undo();
check("an empty undo history falls back instead of no-op-ing", calls.undo === 1);
check(
  "the undo fallback runs execCommand",
  execCalls.length === execBefore + 1 && execCalls[execCalls.length - 1] === "undo",
);
check("the fallback refocuses the remembered editable", window.document.activeElement === input);

inner.focus();
redoDepth = 0;
service.redo();
check("an empty redo history falls back", calls.redo === 1);
check("the redo fallback runs execCommand", execCalls[execCalls.length - 1] === "redo");

input.remove();
button.focus();
const execCount = execCalls.length;
service.undo();
check("a disconnected fallback target no-ops", execCalls.length === execCount);

// --- Registration semantics -----------------------------------------------------------------

const replacement = {
  kind: "prosemirror",
  undo: () => {},
  redo: () => {},
  selectAll: () => {},
};
const stale = registration;
const current = service.register(host, replacement);
stale.dispose();
inner.focus();
check("disposing a stale registration keeps its replacement", service.active === replacement);
check("a prosemirror adapter does not set editorTextFocus", keys().editor === false);
check("a prosemirror adapter sets textInputFocus", keys().text === true);

current.dispose();
check("disposing the current registration unregisters it", service.active === null);
check("unregistering the focused adapter clears editorTextFocus", keys().editor === false);

// --- Dispose ---------------------------------------------------------------------------------

service.dispose();
textarea.focus();
check("a disposed service stops tracking focus", allClear());

contextKeys.dispose();

// --- Edit contribution (src/ui/menu/edit.contribution.ts) ------------------------

// The contribution registers the six edit rows into the shared
// registries at module scope; the commands route through the shared
// text-control singleton.
const editService = getService(TEXT_CONTROL_SERVICE);

const editCommands = [
  ["undo", "Undo"],
  ["redo", "Redo"],
  ["editor.action.clipboardCutAction", "Cut"],
  ["editor.action.clipboardCopyAction", "Copy"],
  ["editor.action.clipboardPasteAction", "Paste"],
  ["editor.action.selectAll", "Select All"],
];
for (const [id, title] of editCommands) {
  const action = Commands.lookup(id);
  check(`the edit contribution registers '${id}' titled '${title}'`, action !== undefined && action.title === title);
  check(`'${id}' gates on the textInputFocus precondition`, action !== undefined && action.precondition === "textInputFocus");
}

const inMenu = (menuId, command, group) =>
  Menus.getMenuItems(menuId).some((row) => row.command === command && row.group === group);
check(
  "undo and redo land in the Edit menu's 1_do group",
  inMenu(MenuId.MenubarEditMenu, "undo", "1_do") && inMenu(MenuId.MenubarEditMenu, "redo", "1_do"),
);
check(
  "cut, copy, and paste land in the Edit menu's 2_ccp group",
  inMenu(MenuId.MenubarEditMenu, "editor.action.clipboardCutAction", "2_ccp") &&
    inMenu(MenuId.MenubarEditMenu, "editor.action.clipboardCopyAction", "2_ccp") &&
    inMenu(MenuId.MenubarEditMenu, "editor.action.clipboardPasteAction", "2_ccp"),
);
check(
  "select-all lands in the Selection menu's 1_basic group",
  inMenu(MenuId.MenubarSelectionMenu, "editor.action.selectAll", "1_basic"),
);

// Command routing: a focused adapter takes undo/redo/select-all.
const editHost = addElement("div");
const editInner = window.document.createElement("div");
editInner.tabIndex = 0;
editHost.appendChild(editInner);

const routed = { undo: 0, redo: 0, selectAll: 0 };
let promptUndoDepth = 1;
editInner.focus();
const editAdapter = editService.register(editHost, {
  kind: "prosemirror",
  undo: () => {
    routed.undo += 1;
  },
  redo: () => {
    routed.redo += 1;
  },
  selectAll: () => {
    routed.selectAll += 1;
  },
  canUndo: () => promptUndoDepth > 0,
});

const routedExecBefore = execCalls.length;
await Commands.execute("undo");
check("the undo command routes to the focused adapter", routed.undo === 1);
await Commands.execute("redo");
check("the redo command routes to the focused adapter", routed.redo === 1);
await Commands.execute("editor.action.selectAll");
check("the select-all command routes to the focused adapter", routed.selectAll === 1);
check("adapter routing never touches execCommand", execCalls.length === routedExecBefore);

// A native editable focused earlier stays the remembered fallback
// target while the adapter holds focus.
const nativeBox = addElement("input");
nativeBox.focus();
editInner.focus();
promptUndoDepth = 0;
await Commands.execute("undo");
check(
  "the undo command falls back to execCommand on an empty adapter history",
  routed.undo === 1 && execCalls[execCalls.length - 1] === "undo",
);
check("the undo fallback refocuses the remembered editable", window.document.activeElement === nativeBox);

// Cut/copy/paste are always the native path, refocused onto the
// remembered editable (a menu click has moved focus since).
const chromeButton = addElement("button");
chromeButton.focus();
await Commands.execute("editor.action.clipboardCutAction");
check(
  "the cut command execCommands on the remembered editable",
  execCalls[execCalls.length - 1] === "cut" && window.document.activeElement === nativeBox,
);
await Commands.execute("editor.action.clipboardCopyAction");
check("the copy command execCommands on the remembered editable", execCalls[execCalls.length - 1] === "copy");
await Commands.execute("editor.action.clipboardPasteAction");
check("the paste command execCommands on the remembered editable", execCalls[execCalls.length - 1] === "paste");

editAdapter.dispose();

if (failures.length > 0) {
  console.error(`text-control-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("text-control-service: all assertions passed");
process.exit(0);
