// Unit test for the keybinding dispatcher
// (src/parts/layout/keybinding-dispatcher.ts): the capture-phase document
// listener that resolves pressed chords through the keybinding registry
// and context-key service. Covers: a bound chord running its command
// and being swallowed before an inner (CodeMirror-shaped) bubble
// listener sees it, unbound keys and modifier-only presses falling
// through untouched, the MoreChordsNeeded waiting status with the
// chordPending context key, the five-second timeout and window-blur
// exits, the three-second not-a-command status for an unrecognized
// second key, a context-gated chord swallowed without running, a
// rejected command reported to the status sink, and the zoom chords
// moved here from test/zoom.mjs (Ctrl+= / Ctrl+Shift+= / Ctrl+- /
// Ctrl+0 through event.code, Alt chords unbound, dispose uninstalling
// the listener). Bundles the module with esbuild and drives it against
// jsdom; the timer exits use mocked timers.
// Run: node --test test/keybinding-dispatcher.mjs
import { mock } from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { KeybindingDispatcher } from "./src/parts/layout/keybinding-dispatcher.ts";
      export { CommandRegistry } from "./src/services/command-registry.ts";
      export { ContextKeyService } from "./src/services/context-key-service.ts";
      export { createKeybindingsRegistry } from "./src/services/keybinding-registry.ts";
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
  loader: { ".css": "empty" },
});
const { KeybindingDispatcher, CommandRegistry, ContextKeyService, createKeybindingsRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Each scenario gets fresh registries, a recording status sink, and its
// own dispatcher, so the capture listener and the chordPending key never
// leak across scenarios.
function scenario() {
  const commands = new CommandRegistry();
  const contextKeys = new ContextKeyService();
  const keybindings = createKeybindingsRegistry("linux");
  const statusLog = [];
  const status = {
    show: (message) => statusLog.push(["show", message]),
    showError: (message) => statusLog.push(["error", message]),
    clear: () => statusLog.push(["clear"]),
  };
  const runs = [];
  const dispatcher = new KeybindingDispatcher({ commands, contextKeys, keybindings, status });
  const press = (key, { target, ...init } = {}) => {
    const event = new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
    (target ?? window.document.body).dispatchEvent(event);
    return event;
  };
  const shown = () => statusLog.filter(([kind]) => kind === "show").map(([, message]) => message);
  return { commands, contextKeys, keybindings, statusLog, runs, press, shown, dispose: () => dispatcher.dispose() };
}

// --- Dispatch: a bound chord runs its command and is consumed ---------------

{
  const s = scenario();
  s.commands.register("test.save", { run: () => s.runs.push("test.save") });
  s.keybindings.registerKeybindingRule({ id: "test.save", keybinding: "ctrl+s" });
  const event = s.press("s", { code: "KeyS", ctrlKey: true });
  check("a bound chord runs its command", s.runs.join(",") === "test.save");
  check("a bound chord is consumed", event.defaultPrevented === true);
  s.dispose();
}

// --- Capture phase: claimed chords never reach an inner listener ------------

{
  const s = scenario();
  s.commands.register("test.save", { run: () => s.runs.push("test.save") });
  s.keybindings.registerKeybindingRule({ id: "test.save", keybinding: "ctrl+s" });
  // A bubble-phase listener on a nested element stands in for
  // CodeMirror's keymap on its content element.
  const editor = window.document.createElement("div");
  window.document.body.append(editor);
  const seen = [];
  editor.addEventListener("keydown", (event) => seen.push(event.key));
  const claimed = s.press("s", { code: "KeyS", ctrlKey: true, target: editor });
  check("a claimed chord is swallowed before the inner listener", seen.length === 0);
  check("a claimed chord is consumed at capture", claimed.defaultPrevented === true);
  check("a claimed chord still runs its command", s.runs.join(",") === "test.save");
  const unbound = s.press("a", { code: "KeyA", target: editor });
  check("an unbound key reaches the inner listener", seen.join(",") === "a");
  check("an unbound key is not consumed", unbound.defaultPrevented === false);
  const modifier = s.press("Control", { code: "ControlLeft", ctrlKey: true, target: editor });
  check("a modifier-only press reaches the inner listener", seen.join(",") === "a,Control");
  check("a modifier-only press is not consumed", modifier.defaultPrevented === false);
  s.dispose();
}

// --- Chords: waiting status, chordPending, completion ------------------------

{
  const s = scenario();
  s.commands.register("test.chord", { run: () => s.runs.push("test.chord") });
  s.keybindings.registerKeybindingRule({ id: "test.chord", keybinding: "ctrl+m ctrl+o" });
  const first = s.press("m", { code: "KeyM", ctrlKey: true });
  check("a chord prefix is consumed", first.defaultPrevented === true);
  check("a chord prefix does not run the command", s.runs.length === 0);
  check("a chord prefix sets chordPending", s.contextKeys.getValue("chordPending") === true);
  check(
    "a chord prefix posts the waiting status",
    s.shown().includes("(Ctrl+M) was pressed. Waiting for second key of chord..."),
  );
  const second = s.press("o", { code: "KeyO", ctrlKey: true });
  check("the second chord key runs the command", s.runs.join(",") === "test.chord");
  check("the second chord key is consumed", second.defaultPrevented === true);
  check("a completed chord clears chordPending", s.contextKeys.getValue("chordPending") === false);
  check("a completed chord clears the waiting status", s.statusLog.some(([kind]) => kind === "clear"));
  s.dispose();
}

// --- Chords: the five-second timeout abandons the pending chord --------------

{
  mock.timers.enable({ apis: ["setTimeout"] });
  const s = scenario();
  s.commands.register("test.chord", { run: () => s.runs.push("test.chord") });
  s.keybindings.registerKeybindingRule({ id: "test.chord", keybinding: "ctrl+m ctrl+o" });
  s.press("m", { code: "KeyM", ctrlKey: true });
  mock.timers.tick(4999);
  check("the chord waits the full five seconds", s.contextKeys.getValue("chordPending") === true);
  mock.timers.tick(1);
  check("a five-second timeout abandons the chord", s.contextKeys.getValue("chordPending") === false);
  check("a timeout clears the waiting status", s.statusLog.some(([kind]) => kind === "clear"));
  const after = s.press("o", { code: "KeyO", ctrlKey: true });
  check("the second key after a timeout is not the chord", s.runs.length === 0);
  check("the second key after a timeout falls through", after.defaultPrevented === false);
  s.dispose();
  mock.timers.reset();
}

// --- Chords: window blur abandons the pending chord --------------------------

{
  const s = scenario();
  s.commands.register("test.chord", { run: () => s.runs.push("test.chord") });
  s.keybindings.registerKeybindingRule({ id: "test.chord", keybinding: "ctrl+m ctrl+o" });
  s.press("m", { code: "KeyM", ctrlKey: true });
  window.dispatchEvent(new window.Event("blur"));
  check("window blur abandons the chord", s.contextKeys.getValue("chordPending") === false);
  check("window blur clears the waiting status", s.statusLog.some(([kind]) => kind === "clear"));
  const after = s.press("o", { code: "KeyO", ctrlKey: true });
  check("the second key after blur is not the chord", s.runs.length === 0);
  check("the second key after blur falls through", after.defaultPrevented === false);
  s.dispose();
}

// --- Chords: an unrecognized second key posts the not-a-command status -------

{
  mock.timers.enable({ apis: ["setTimeout"] });
  const s = scenario();
  s.commands.register("test.chord", { run: () => s.runs.push("test.chord") });
  s.keybindings.registerKeybindingRule({ id: "test.chord", keybinding: "ctrl+m ctrl+o" });
  s.press("m", { code: "KeyM", ctrlKey: true });
  const stray = s.press("x", { code: "KeyX" });
  check("an unrecognized second key is consumed", stray.defaultPrevented === true);
  check("an unrecognized second key abandons the chord", s.contextKeys.getValue("chordPending") === false);
  check(
    "an unrecognized chord posts the not-a-command status",
    s.shown().includes("The key combination (Ctrl+M, X) is not a command."),
  );
  check("the not-a-command status has not cleared yet", !s.statusLog.some(([kind]) => kind === "clear"));
  mock.timers.tick(3000);
  check("the not-a-command status clears after three seconds", s.statusLog.some(([kind]) => kind === "clear"));
  s.dispose();
  mock.timers.reset();
}

// --- Context gating: a claimed chord is swallowed even when its when fails ---

{
  const s = scenario();
  const canRun = s.contextKeys.createKey("canRun", false);
  s.commands.register("test.gated", { run: () => s.runs.push("test.gated") });
  s.keybindings.registerKeybindingRule({ id: "test.gated", keybinding: "ctrl+k", when: "canRun" });
  const blocked = s.press("k", { code: "KeyK", ctrlKey: true });
  check("a context-gated chord is still swallowed", blocked.defaultPrevented === true);
  check("a context-gated chord does not run", s.runs.length === 0);
  canRun.set(true);
  s.press("k", { code: "KeyK", ctrlKey: true });
  check("the chord runs once its when passes", s.runs.join(",") === "test.gated");
  s.dispose();
}

// --- Failure: a rejected command reports to the status sink ------------------

{
  const s = scenario();
  s.commands.register("test.boom", { run: () => Promise.reject(new Error("boom")) });
  s.keybindings.registerKeybindingRule({ id: "test.boom", keybinding: "ctrl+e" });
  const event = s.press("e", { code: "KeyE", ctrlKey: true });
  check("a failing command's chord is still consumed", event.defaultPrevented === true);
  for (let i = 0; i < 5; i += 1) {
    await Promise.resolve();
  }
  check(
    "a failing command posts an error status",
    s.statusLog.some(([kind, message]) => kind === "error" && message === "Could not run 'test.boom': boom"),
  );
  s.dispose();
}

// --- Zoom chords (moved from test/zoom.mjs): Ctrl+= / Ctrl+Shift+= / Ctrl+- / Ctrl+0

{
  const s = scenario();
  s.commands.register("chrome.zoomIn", { run: () => s.runs.push("zoomIn") });
  s.commands.register("chrome.zoomOut", { run: () => s.runs.push("zoomOut") });
  s.commands.register("chrome.resetZoom", { run: () => s.runs.push("resetZoom") });
  // The zoom-in pair: the shifted plus key is the same physical key, so
  // both chords bind the command (VS Code binds ctrl+= and ctrl+shift+=).
  s.keybindings.registerKeybindingRule({ id: "chrome.zoomIn", keybinding: "ctrlcmd+=" });
  s.keybindings.registerKeybindingRule({ id: "chrome.zoomIn", keybinding: "ctrlcmd+shift+=" });
  s.keybindings.registerKeybindingRule({ id: "chrome.zoomOut", keybinding: "ctrlcmd+-" });
  s.keybindings.registerKeybindingRule({ id: "chrome.resetZoom", keybinding: "ctrlcmd+0" });
  let event = s.press("=", { code: "Equal", ctrlKey: true });
  check("Ctrl+= dispatches zoom in", s.runs.at(-1) === "zoomIn");
  check("Ctrl+= is consumed", event.defaultPrevented === true);
  event = s.press("+", { code: "Equal", ctrlKey: true, shiftKey: true });
  check("Ctrl+Shift+= dispatches zoom in through the shifted + key", s.runs.at(-1) === "zoomIn");
  check("Ctrl+Shift+= is consumed", event.defaultPrevented === true);
  event = s.press("-", { code: "Minus", ctrlKey: true });
  check("Ctrl+- dispatches zoom out", s.runs.at(-1) === "zoomOut");
  check("Ctrl+- is consumed", event.defaultPrevented === true);
  event = s.press("0", { code: "Digit0", ctrlKey: true });
  check("Ctrl+0 dispatches reset zoom", s.runs.at(-1) === "resetZoom");
  check("Ctrl+0 is consumed", event.defaultPrevented === true);
  event = s.press("=", { code: "Equal", ctrlKey: true, altKey: true });
  check("an Alt chord is not a zoom binding", s.runs.at(-1) === "resetZoom");
  check("an Alt chord is not consumed", event.defaultPrevented === false);
  s.dispose();
  event = s.press("=", { code: "Equal", ctrlKey: true });
  check("disposing uninstalls the keydown listener", s.runs.at(-1) === "resetZoom");
  check("a disposed dispatcher no longer consumes", event.defaultPrevented === false);
}

if (failures.length > 0) {
  console.error(`keybinding-dispatcher: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("keybinding-dispatcher: all assertions passed");
