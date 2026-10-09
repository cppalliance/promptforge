// Unit test for the editor settings service and its surface wiring
// (plan step 14, src/parts/editor/editor-settings-service.ts,
// editor-surface.ts, editor.contribution.ts). The service seeds four
// boolean settings from the UI-state adapter's user bucket behind a
// shape check, writes every change back through the adapter, publishes
// them as the config.editor.* context keys, and fires onDidChange; the
// CodeMirror surface holds one Compartment per setting and follows
// the service in place - no state rebuild. The contribution's four
// toggle actions are asserted on the shared registries. Bundles the
// modules with esbuild and drives them in jsdom with the same
// measurement shims as editor-idioms.mjs.
// Run: node --test test/editor-settings.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/parts/editor/editor.contribution.ts";
      export { CodeMirrorSurface } from "./src/parts/editor/editor-surface.ts";
      export { EditorSettingsService } from "./src/parts/editor/editor-settings-service.ts";
      export { DEFAULT_EDITOR_SETTINGS, EDITOR_SETTINGS_SERVICE } from "./src/services/editor-settings-service.ts";
      export { ContextKeyService } from "@workshop/platform/context-key-service";
      export { getService } from "@workshop/platform/service-registry";
      export { Commands } from "@workshop/platform/command-registry";
      export { Menus, MenuId } from "@workshop/platform/menu-registry";
      export { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
      export { EditorView } from "@codemirror/view";
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
  // The modules under test import colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7913/",
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
  "HTMLInputElement",
  "Node",
  "Element",
  "Range",
  "Event",
  "CustomEvent",
  "MutationObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
  "localStorage",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;

// The contribution registers at module scope; a malformed descriptor
// reports through console.error, so spy on it across the bundle import.
const consoleErrors = [];
const realConsoleError = console.error;
console.error = (...args) => {
  consoleErrors.push(args.join(" "));
};

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-settings-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  CodeMirrorSurface,
  DEFAULT_EDITOR_SETTINGS,
  EDITOR_SETTINGS_SERVICE,
  EditorSettingsService,
  ContextKeyService,
  getService,
  Commands,
  Menus,
  MenuId,
  KeybindingsRegistry,
  EditorView,
} = await import(pathToFileURL(bundlePath).href);
console.error = realConsoleError;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** The user-bucket key the composition root binds the service to. */
const KEY = "editor_settings";

/**
 * Builds a service over a fake adapter the way main.ts binds the live one:
 * the initial value read from the user bucket, every change written back
 * to the same key. Returns the service and the fake, whose `sets` records
 * the writes.
 */
function serviceOver(initial, contextKeys = null) {
  const storage = createFakeUiStorage(initial === undefined ? {} : { user: { [KEY]: initial } });
  const service = new EditorSettingsService(
    storage.get("user", KEY),
    (value) => storage.set("user", KEY, value),
    contextKeys,
  );
  return { service, storage };
}

{
  // Defaults, toggle, the write-through, and the no-op set.
  const { service, storage } = serviceOver();
  check("wordWrap defaults off", service.settings.wordWrap === false);
  check("renderWhitespace defaults to selection, as in Cursor", service.settings.renderWhitespace === "selection");
  check("the exported renderWhitespace default is selection", DEFAULT_EDITOR_SETTINGS.renderWhitespace === "selection");
  check("renderControlCharacters defaults on, as in Cursor", service.settings.renderControlCharacters === true);
  check("columnSelection defaults off", service.settings.columnSelection === false);
  check("the exported defaults match", DEFAULT_EDITOR_SETTINGS.renderControlCharacters === true);
  check("construction writes nothing", storage.sets.length === 0);

  let fired = 0;
  let last = null;
  service.onDidChange((settings) => { fired += 1; last = settings; });
  service.toggle("wordWrap");
  check("toggle flips the setting", service.settings.wordWrap === true);
  check("toggle fires onDidChange with the new settings", fired === 1 && last !== null && last.wordWrap === true);
  check(
    "toggle writes the whole settings object to the user bucket",
    storage.sets.length === 1 &&
      storage.sets[0].bucket === "user" &&
      storage.sets[0].key === KEY &&
      storage.sets[0].value.wordWrap === true &&
      storage.sets[0].value.renderControlCharacters === true,
  );

  service.set("wordWrap", true);
  check("set to the current value is a no-op - no event, no write", fired === 1 && storage.sets.length === 1);

  // A relaunch: a new service seeded from what the last write stored.
  const reloaded = new EditorSettingsService(storage.get("user", KEY), () => {}, null);
  check("a new service over the written value reads it back", reloaded.settings.wordWrap === true);

  const { service: fromMalformed } = serviceOver("{not json");
  check(
    "a non-object initial value reads as the defaults, never a cast",
    fromMalformed.settings.wordWrap === false && fromMalformed.settings.renderControlCharacters === true,
  );
  const { service: fromArray } = serviceOver([true, true, true, true]);
  check("an array initial value reads as the defaults", fromArray.settings.wordWrap === false);
  const { service: fromWrongTypes } = serviceOver({ wordWrap: "yes", columnSelection: 1, renderWhitespace: true });
  check(
    "non-boolean fields drop to defaults while real booleans survive (a legacy true reads as all)",
    fromWrongTypes.settings.wordWrap === false &&
      fromWrongTypes.settings.columnSelection === false &&
      fromWrongTypes.settings.renderWhitespace === "all",
  );
  // The three render-whitespace modes, and the legacy boolean a stored
  // value may still hold: true was "on" (all), false was "off", which now
  // reads as the default (selection).
  const modeOf = (stored) => serviceOver({ renderWhitespace: stored }).service.settings.renderWhitespace;
  check("a stored none reads as none", modeOf("none") === "none");
  check("a stored selection reads as selection", modeOf("selection") === "selection");
  check("a stored all reads as all", modeOf("all") === "all");
  check("a legacy stored true reads as all", modeOf(true) === "all");
  check("a legacy stored false reads as selection", modeOf(false) === "selection");
  check("an unknown stored string reads as the default", modeOf("sometimes") === "selection");
  check("a stored number reads as the default", modeOf(1) === "selection");
  check("a missing key reads as the default", serviceOver({}).service.settings.renderWhitespace === "selection");

  // Choosing a mode writes the mode string, never a boolean.
  const { service: modal, storage: modalStorage } = serviceOver();
  modal.set("renderWhitespace", "all");
  check(
    "set takes a mode and writes it to the user bucket",
    modal.settings.renderWhitespace === "all" && modalStorage.sets.at(-1)?.value.renderWhitespace === "all",
  );
  modal.set("renderWhitespace", "all");
  check("set to the current mode is a no-op write", modalStorage.sets.length === 1);

  // The Render Whitespace toggle flips between none and all; the default
  // selection reads as on, so the first flip turns it off.
  const { service: flipper } = serviceOver();
  flipper.toggle("renderWhitespace");
  check("toggling from the default selection goes to none", flipper.settings.renderWhitespace === "none");
  flipper.toggle("renderWhitespace");
  check("toggling from none goes to all", flipper.settings.renderWhitespace === "all");
  flipper.toggle("renderWhitespace");
  check("toggling from all goes to none", flipper.settings.renderWhitespace === "none");
  modal.dispose();
  flipper.dispose();

  service.dispose();
  reloaded.dispose();
  fromMalformed.dispose();
  fromArray.dispose();
  fromWrongTypes.dispose();
}

{
  // A writer that fails leaves the in-memory settings authoritative: the
  // toggle still lands, the event still fires, and nothing escapes.
  let writes = 0;
  const service = new EditorSettingsService(
    null,
    () => {
      writes += 1;
      throw new Error("denied");
    },
    null,
  );
  let fired = 0;
  service.onDidChange(() => { fired += 1; });
  let escaped = false;
  try {
    service.toggle("columnSelection");
  } catch {
    escaped = true;
  }
  check("a failing writer does not escape the toggle", escaped === false);
  check("a failing writer still receives the attempt", writes === 1);
  check("a failing writer leaves the new value in place", service.settings.columnSelection === true);
  check("a failing writer still fires onDidChange", fired === 1);
  service.dispose();
}

{
  // The config.editor.* context keys follow the service.
  const contextKeys = new ContextKeyService();
  const { service } = serviceOver({ wordWrap: true }, contextKeys);
  check(
    "construction publishes the defaults - the render-whitespace key is true under the default selection mode",
    contextKeys.getValue("config.editor.renderControlCharacters") === true &&
      contextKeys.getValue("config.editor.renderWhitespace") === true &&
      contextKeys.getValue("config.editor.columnSelection") === false,
  );
  check(
    "construction publishes the persisted value over the default",
    contextKeys.getValue("config.editor.wordWrap") === true,
  );
  service.toggle("renderWhitespace");
  check(
    "toggling Render Whitespace off clears its boolean key",
    contextKeys.getValue("config.editor.renderWhitespace") === false,
  );
  service.toggle("renderWhitespace");
  check("toggling back sets the boolean key (mode all)", contextKeys.getValue("config.editor.renderWhitespace") === true);
  service.set("renderWhitespace", "selection");
  check(
    "the key stays boolean true for the selection mode",
    contextKeys.getValue("config.editor.renderWhitespace") === true,
  );
  service.set("renderWhitespace", "none");
  check("only the none mode clears the key", contextKeys.getValue("config.editor.renderWhitespace") === false);
  service.dispose();

  // A persisted none publishes false over the true default.
  const offKeys = new ContextKeyService();
  const { service: persistedOff } = serviceOver({ renderWhitespace: "none" }, offKeys);
  check(
    "a persisted none publishes a false key at construction",
    offKeys.getValue("config.editor.renderWhitespace") === false,
  );
  persistedOff.dispose();
}

{
  // The surface's compartments follow the service in place.
  const contextKeys = new ContextKeyService();
  const { service } = serviceOver(undefined, contextKeys);
  const surface = new CodeMirrorSurface(service);
  window.document.body.appendChild(surface.element);
  surface.open({ path: "C:\\project\\c.txt", text: "alpha beta\nthird line\n" });
  const view = surface.editorView();

  const wraps = () =>
    view.state
      .facet(EditorView.contentAttributes)
      .some((value) => typeof value !== "function" && value.class === "cm-lineWrapping");
  check("word wrap starts off", !wraps());
  service.toggle("wordWrap");
  check("toggling wordWrap reconfigures the lineWrapping compartment", wraps());
  check("the wordWrap context key follows the surface's service", contextKeys.getValue("config.editor.wordWrap") === true);
  check("a reconfigure keeps the same view - no state rebuild", surface.editorView() === view);
  check("a reconfigure keeps the document text", surface.text() === "alpha beta\nthird line\n");
  service.toggle("wordWrap");
  check("toggling wordWrap off reconfigures back", !wraps());

  // Render Whitespace: the default selection mode marks whitespace inside
  // the selection only; all marks every space and tab; none marks nothing.
  const spaces = () => view.contentDOM.querySelectorAll(".cm-highlightSpace").length;
  const tabs = () => view.contentDOM.querySelectorAll(".cm-highlightTab").length;
  check("the default is the selection mode", service.settings.renderWhitespace === "selection");
  check("selection mode marks nothing while no text is selected", spaces() === 0);
  view.dispatch({ selection: { anchor: 0, head: 10 } });
  check("selection mode marks the space inside a selection", spaces() === 1);
  view.dispatch({ selection: { anchor: 0, head: 5 } });
  check("selection mode marks nothing when the selection holds no whitespace", spaces() === 0);
  view.dispatch({ selection: { anchor: 11, head: 21 } });
  check("selection mode marks only the selected line's whitespace", spaces() === 1);
  view.dispatch({ selection: { anchor: 3, head: 3 } });
  check("selection mode clears when the selection collapses", spaces() === 0);

  service.set("renderWhitespace", "all");
  check("all marks every space in the document", spaces() === 2);
  service.toggle("renderWhitespace");
  check("toggling from all removes the highlights", spaces() === 0);
  view.dispatch({ selection: { anchor: 0, head: 21 } });
  check("none marks nothing even inside a selection", spaces() === 0);
  service.toggle("renderWhitespace");
  check("toggling from none highlights every space again", spaces() === 2);

  // Tabs follow the same rules as spaces.
  surface.open({ path: "C:\\project\\tabs.txt", text: "a\tb\n" });
  check("all marks a tab", tabs() === 1);
  service.set("renderWhitespace", "selection");
  view.dispatch({ selection: { anchor: 0, head: 3 } });
  check("selection mode marks a selected tab", tabs() === 1);
  view.dispatch({ selection: { anchor: 0, head: 1 } });
  check("selection mode leaves an unselected tab alone", tabs() === 0);
  surface.open({ path: "C:\\project\\c.txt", text: "alpha beta\nthird line\n" });
  service.set("renderWhitespace", "selection");

  // Control characters: on by default, over a document holding one.
  surface.open({ path: "C:\\project\\c.txt", text: "a\u0001b\n" });
  check(
    "control characters render by default",
    view.contentDOM.querySelector(".cm-specialChar") !== null,
  );
  service.toggle("renderControlCharacters");
  check(
    "toggling renderControlCharacters off drops the rendering",
    view.contentDOM.querySelector(".cm-specialChar") === null,
  );
  service.toggle("renderControlCharacters");
  check(
    "toggling renderControlCharacters back on restores it",
    view.contentDOM.querySelector(".cm-specialChar") !== null,
  );

  // Column selection: the mouseSelectionStyle facet answers whether a
  // drag gesture starts a rectangular selection. Cursor's gesture is
  // Shift+Alt+drag, because a plain Alt+click adds a cursor; the off state
  // keeps exactly that one.
  const plainDrag = { altKey: false, shiftKey: false, button: 0, clientX: 0, clientY: 0 };
  const altDrag = { altKey: true, shiftKey: false, button: 0, clientX: 0, clientY: 0 };
  const shiftAltDrag = { altKey: true, shiftKey: true, button: 0, clientX: 0, clientY: 0 };
  const styles = () => view.state.facet(EditorView.mouseSelectionStyle);
  check(
    "Shift+Alt+drag starts a rectangular selection",
    styles().some((style) => style(view, shiftAltDrag) !== null),
  );
  check(
    "a plain Alt+drag is not rectangular - Alt+click adds a cursor instead",
    styles().every((style) => style(view, altDrag) === null),
  );
  check(
    "a plain drag is not rectangular by default",
    styles().every((style) => style(view, plainDrag) === null),
  );
  service.toggle("columnSelection");
  check(
    "column selection makes every left drag rectangular",
    styles().some((style) => style(view, plainDrag) !== null),
  );
  service.toggle("columnSelection");
  check(
    "toggling column selection off restores the Shift+Alt gesture only",
    styles().every((style) => style(view, plainDrag) === null) &&
      styles().some((style) => style(view, shiftAltDrag) !== null),
  );

  // A surface created after a change seeds its first state from the
  // service's current settings.
  service.set("wordWrap", true);
  const seeded = new CodeMirrorSurface(service);
  window.document.body.appendChild(seeded.element);
  seeded.open({ path: "C:\\project\\d.txt", text: "seeded\n" });
  check(
    "a new surface's first state applies the current settings",
    seeded
      .editorView()
      .state.facet(EditorView.contentAttributes)
      .some((value) => typeof value !== "function" && value.class === "cm-lineWrapping"),
  );
  seeded.dispose();
  surface.dispose();
  service.dispose();
}

{
  // The contribution's four toggle actions on the shared registries.
  check("the contribution registered without errors", consoleErrors.length === 0);
  const TOGGLES = [
    ["editor.action.toggleColumnSelection", "config.editor.columnSelection"],
    ["editor.action.toggleWordWrap", "config.editor.wordWrap"],
    ["editor.action.toggleRenderWhitespace", "config.editor.renderWhitespace"],
    ["editor.action.toggleRenderControlCharacter", "config.editor.renderControlCharacters"],
  ];
  for (const [id, toggled] of TOGGLES) {
    check(`${id} is a registered command`, Commands.lookup(id) !== undefined);
    check(`${id} declares its toggled expression`, Commands.lookup(id)?.toggled === toggled);
  }
  const palette = Menus.getMenuItems(MenuId.CommandPalette).map((row) => row.command);
  check(
    "every toggle reaches the palette",
    TOGGLES.every(([id]) => palette.includes(id)),
  );
  const viewMenu = Menus.getMenuItems(MenuId.MenubarViewMenu).map((row) => row.command);
  check("Word Wrap lands in the View menu", viewMenu.includes("editor.action.toggleWordWrap"));
  const appearance = Menus.getMenuItems("menubar/view/appearance").map((row) => row.command);
  check(
    "the render toggles land in the Appearance menu",
    appearance.includes("editor.action.toggleRenderWhitespace") &&
      appearance.includes("editor.action.toggleRenderControlCharacter"),
  );
  const selectionMenu = Menus.getMenuItems(MenuId.MenubarSelectionMenu).map((row) => row.command);
  check(
    "Column Selection Mode lands in the Selection menu",
    selectionMenu.includes("editor.action.toggleColumnSelection"),
  );
  check(
    "Word Wrap binds Alt+Z",
    KeybindingsRegistry.lookupKeybinding("editor.action.toggleWordWrap") !== undefined,
  );
  check(
    "Render Whitespace has no keybinding",
    KeybindingsRegistry.lookupKeybinding("editor.action.toggleRenderWhitespace") === undefined,
  );
  check(
    "Column Selection Mode keeps the editor-only precondition",
    Commands.lookup("editor.action.toggleColumnSelection")?.precondition === "activeEditor == 'editor'",
  );
  check(
    "Word Wrap has no precondition - it toggles without an active editor",
    Commands.lookup("editor.action.toggleWordWrap")?.precondition === undefined,
  );

  // Executing through the command registry flips the shared service.
  const shared = getService(EDITOR_SETTINGS_SERVICE);
  check(
    "the Render Whitespace row reads as checked by default: its mode is not none",
    shared.settings.renderWhitespace === "selection",
  );
  await Commands.execute("editor.action.toggleRenderWhitespace");
  check("executing the Render Whitespace row turns it off", shared.settings.renderWhitespace === "none");
  await Commands.execute("editor.action.toggleRenderWhitespace");
  check("executing it again turns it all the way on", shared.settings.renderWhitespace === "all");
  shared.set("renderWhitespace", "selection");
  const before = shared.settings.wordWrap;
  const executed = await Commands.execute("editor.action.toggleWordWrap");
  check(
    "executing the toggle flips the shared settings service",
    executed === true && shared.settings.wordWrap === !before,
  );
  await Commands.execute("editor.action.toggleWordWrap");
}

if (failures.length > 0) {
  console.error(`editor-settings: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-settings: all assertions passed");
process.exit(0);
