// The chat pane's chords and commands (src/parts/agent/agent.contribution.ts and
// agent-commands.ts) against the real registries, the real keybinding
// dispatcher, and a real Dockview dock in jsdom. Two layers:
// - The chords: each key press goes through the dispatcher with a recording
//   command registry, so the test reads which command a chord reaches under
//   which context (activeEditor, editorTextFocus) without running it.
//   Ctrl+Shift+Backspace stops and Escape does not; Ctrl+L and Ctrl+I open
//   the chat and Ctrl+Shift+L and Ctrl+Shift+I start one, beating the Go
//   menu's Add Symbol stubs, which carry no chord any more; Ctrl+T and Ctrl+N
//   open a chat tab while a chat is active, and Ctrl+N stays New Text File
//   while the editor's own text is focused; Ctrl+[ and Ctrl+] cycle chat
//   tabs, and while the editor's text is focused they stay Indent Line and
//   Outdent Line instead of being swallowed; Ctrl+W closes one;
//   Ctrl+. and Ctrl+/ open the composer's menus
//   (Ctrl+/ beating Toggle Line Comment); Ctrl+Shift+Space is Voice Input;
//   and Shift+Tab is no chord at all, so it keeps moving focus backwards
//   everywhere else.
// - The commands: Open Chat hides the pane when the chat has focus and
//   otherwise reveals it and focuses the composer; New Chat reuses an empty
//   chat or opens one; Stop reaches the active chat; the tab cycle walks the
//   group's chats; Close Tab hides the pane when it closes the last chat.
// Run: node test/agent-keybindings.mjs
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
      import "./src/parts/editor/closed-editors.ts";
      import "./src/parts/workspace/workspace.contribution.ts";
      import "./src/parts/editor/editor.contribution.ts";
      import "./src/parts/agent/agent.contribution.ts";
      import "./src/parts/layout/layout.contribution.ts";
      import "./src/parts/menu/stubs.contribution.ts";
      export { createDockview, themeDark } from "dockview";
      export { Commands } from "@workshop/platform/command-registry";
      export { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
      export { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
      export { resolvePanelContent } from "@workshop/platform/panel-registry";
      export { getService } from "@workshop/platform/service-registry";
      export { KeybindingDispatcher } from "./src/parts/layout/keybinding-dispatcher.ts";
      export { bindActiveEditorKey, groupOfZone, initZones, openInZone } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent, PANEL_TAB } from "./src/parts/layout/panel-types.ts";
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

const dom = new JSDOM('<!doctype html><html><body><div id="dock"></div></body></html>', {
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

globalThis.fetch = async (url) => {
  throw new Error(`unexpected fetch in the agent-keybindings test: ${url}`);
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
// The agent panel composes an AgentSocket on init; a socket that never
// opens keeps it inert - this test drives the commands, not the wire.
globalThis.WebSocket = class {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  readyState = 0;
  send() {}
  close() {}
};
if (typeof window.crypto?.randomUUID !== "function") {
  let serial = 0;
  Object.defineProperty(window, "crypto", {
    configurable: true,
    value: { randomUUID: () => `uuid-${(serial += 1)}` },
  });
}

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-keybindings-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  Commands,
  CONTEXT_KEY_SERVICE,
  KeybindingsRegistry,
  resolvePanelContent,
  getService,
  KeybindingDispatcher,
  bindActiveEditorKey,
  groupOfZone,
  initZones,
  openInZone,
  createPanelComponent,
  createPanelTabComponent,
  PANEL_TAB,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 6; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const contextKeys = getService(CONTEXT_KEY_SERVICE);
const activeEditorKey = contextKeys.createKey("activeEditor", undefined);
const editorTextFocusKey = contextKeys.createKey("editorTextFocus", false);

// --- The chords: which command each press reaches ---------------------------------------

const executed = [];
const dispatcher = new KeybindingDispatcher({
  commands: {
    execute: (id) => {
      executed.push(id);
      return Promise.resolve();
    },
  },
  status: { show() {}, showError() {}, clear() {} },
});

const CODES = {
  l: "KeyL",
  i: "KeyI",
  t: "KeyT",
  n: "KeyN",
  w: "KeyW",
  ".": "Period",
  "/": "Slash",
  "[": "BracketLeft",
  "]": "BracketRight",
  backspace: "Backspace",
  space: "Space",
  escape: "Escape",
  tab: "Tab",
};

/** Presses a chord and answers the command it reached, or null. */
async function press(key, { activeEditor, textFocus = false, shift = false, ctrl = true } = {}) {
  activeEditorKey.set(activeEditor);
  editorTextFocusKey.set(textFocus);
  executed.length = 0;
  const event = new window.KeyboardEvent("keydown", {
    key,
    code: CODES[key],
    ctrlKey: ctrl,
    shiftKey: shift,
    bubbles: true,
    cancelable: true,
  });
  window.document.dispatchEvent(event);
  await flush();
  return { command: executed[0] ?? null, consumed: event.defaultPrevented };
}

{
  const stop = await press("backspace", { activeEditor: "agent", shift: true });
  check("Ctrl+Shift+Backspace in a chat is composer.cancelComposerStep", stop.command === "composer.cancelComposerStep");
  const elsewhere = await press("backspace", { activeEditor: "editor", shift: true });
  check("Ctrl+Shift+Backspace outside a chat reaches nothing", elsewhere.command === null);
  const escape = await press("escape", { activeEditor: "agent", ctrl: false });
  check("Escape in a chat stops nothing and reaches no command", escape.command === null && escape.consumed === false);
  check(
    "no keybinding rule binds Escape to the stop command",
    !KeybindingsRegistry.getResolver().hasRuleForChord({ ctrl: false, shift: false, alt: false, meta: false, key: "escape" }),
  );

  check("Ctrl+L opens the chat from anywhere", (await press("l", { activeEditor: "editor" })).command === "workbench.action.chat.open");
  check("Ctrl+I opens the chat from anywhere", (await press("i", { activeEditor: "editor" })).command === "workbench.action.chat.open");
  check(
    "Ctrl+Shift+L starts a chat from anywhere",
    (await press("l", { activeEditor: "editor", shift: true })).command === "workbench.action.chat.new",
  );
  check(
    "Ctrl+Shift+I starts a chat from anywhere",
    (await press("i", { activeEditor: "editor", shift: true })).command === "workbench.action.chat.new",
  );
  check(
    "the Go menu's Add Symbol rows no longer carry the chords the chat took",
    KeybindingsRegistry.lookupKeybinding("workbench.action.addSymbolToCurrentChat") === undefined &&
      KeybindingsRegistry.lookupKeybinding("workbench.action.addSymbolToNewChat") === undefined,
  );
  check(
    "Open Chat and New Chat show their first chords as labels",
    KeybindingsRegistry.lookupKeybinding("workbench.action.chat.open")?.getLabel() === "Ctrl+L" &&
      KeybindingsRegistry.lookupKeybinding("workbench.action.chat.new")?.getLabel() === "Ctrl+Shift+L",
  );

  check(
    "Ctrl+T in a chat is New Chat Tab",
    (await press("t", { activeEditor: "agent" })).command === "workbench.action.chat.newTab",
  );
  check(
    "Ctrl+T outside a chat reaches no command (the Go to Symbol stub is disabled)",
    (await press("t", { activeEditor: "editor" })).command === null,
  );
  check(
    "Ctrl+N in a chat is New Chat Tab",
    (await press("n", { activeEditor: "agent" })).command === "workbench.action.chat.newTab",
  );
  check(
    "Ctrl+N stays New Text File while the editor's text is focused",
    (await press("n", { activeEditor: "agent", textFocus: true })).command === "workbench.action.files.newUntitledFile",
  );
  check(
    "Ctrl+N outside a chat is New Text File",
    (await press("n", { activeEditor: "editor" })).command === "workbench.action.files.newUntitledFile",
  );

  check("Ctrl+] in a chat cycles to the next tab", (await press("]", { activeEditor: "agent" })).command === "workbench.action.chat.nextTab");
  check("Ctrl+[ in a chat cycles to the previous tab", (await press("[", { activeEditor: "agent" })).command === "workbench.action.chat.previousTab");
  // The editor's indent chords: the dispatcher swallows a claimed chord even
  // where its `when` fails, so the editor must own Ctrl+] and Ctrl+[ through
  // rules of its own or CodeMirror's indentMore and indentLess never see them.
  const indent = await press("]", { activeEditor: "editor", textFocus: true });
  check(
    "Ctrl+] with the editor's text focused is Indent Line, not swallowed with no command",
    indent.command === "editor.action.indentLines" && indent.consumed === true,
  );
  const outdent = await press("[", { activeEditor: "editor", textFocus: true });
  check(
    "Ctrl+[ with the editor's text focused is Outdent Line, not swallowed with no command",
    outdent.command === "editor.action.outdentLines" && outdent.consumed === true,
  );
  check(
    "Ctrl+] and Ctrl+[ with an editor active but its text not focused reach no command",
    (await press("]", { activeEditor: "editor" })).command === null &&
      (await press("[", { activeEditor: "editor" })).command === null,
  );
  check("Ctrl+W in a chat closes the tab", (await press("w", { activeEditor: "agent" })).command === "workbench.action.chat.closeTab");
  check("Ctrl+. in a chat opens the mode menu", (await press(".", { activeEditor: "agent" })).command === "composer.openModeMenu");
  check("Ctrl+/ in a chat opens the model menu", (await press("/", { activeEditor: "agent" })).command === "composer.openModelMenu");
  check(
    "Ctrl+/ in the editor is still Toggle Line Comment",
    (await press("/", { activeEditor: "editor", textFocus: true })).command === "editor.action.commentLine",
  );
  check(
    "Ctrl+Shift+Space in a chat is Voice Input",
    (await press("space", { activeEditor: "agent", shift: true })).command === "workbench.action.chat.toggleVoiceInput",
  );
  check(
    "Shift+Tab is no registered chord, so no control loses its way backwards",
    !KeybindingsRegistry.getResolver().hasRuleForChord({ ctrl: false, shift: true, alt: false, meta: false, key: "tab" }),
  );
  const shiftTab = await press("tab", { activeEditor: "agent", shift: true, ctrl: false });
  check("Shift+Tab reaches no command and is not consumed", shiftTab.command === null && shiftTab.consumed === false);
}
dispatcher.dispose();

// --- The commands against a real dock --------------------------------------------------

const dock = createDockview(window.document.getElementById("dock"), {
  createComponent: createPanelComponent,
  createTabComponent: createPanelTabComponent,
  defaultTabComponent: PANEL_TAB,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
initZones(dock);
bindActiveEditorKey(dock);
dock.layout(1200, 800);

/** The real panel behind a dock panel, its handle methods replaceable by spies. */
const realPanelOf = (panel) => resolvePanelContent(panel.view.content);

const first = openInZone("agent", { instance: "one" });
await flush();
const firstPanel = realPanelOf(first);
check("the first chat's panel mounted, so its handle is live", typeof firstPanel.focusInput === "function");

// Spies stand in for what a live session reports.
const spies = new Map();
function spyOn(panel, state) {
  const record = { focus: 0, cancels: 0, voice: 0, modeMenus: 0, modelMenus: 0, ...state };
  spies.set(panel, record);
  panel.focusInput = () => {
    record.focus += 1;
  };
  panel.hasFocus = () => record.hasFocus;
  panel.isEmpty = () => record.empty;
  panel.cancelTurn = () => {
    record.cancels += 1;
    return true;
  };
  panel.toggleVoiceInput = () => {
    record.voice += 1;
  };
  panel.openModeMenu = () => {
    record.modeMenus += 1;
  };
  panel.openModelMenu = () => {
    record.modelMenus += 1;
  };
  return record;
}
const firstSpy = spyOn(firstPanel, { hasFocus: false, empty: false });

// Stop, voice, and the menus reach the active chat only.
first.api.setActive();
await Commands.execute("composer.cancelComposerStep");
await Commands.execute("workbench.action.chat.toggleVoiceInput");
await Commands.execute("composer.openModeMenu");
await Commands.execute("composer.openModelMenu");
check(
  "Stop, Voice Input, and both menus reach the active chat",
  firstSpy.cancels === 1 && firstSpy.voice === 1 && firstSpy.modeMenus === 1 && firstSpy.modelMenus === 1,
);

// Open Chat: focus in the chat hides the pane; without focus it reveals the pane and focuses the composer.
firstSpy.hasFocus = true;
await Commands.execute("workbench.action.chat.open");
check("Open Chat with the chat focused hides the pane", groupOfZone("right")?.api.isVisible === false);
check("hiding the pane clears the Secondary Side Bar key", contextKeys.getValue("auxiliaryBarVisible") === false);
firstSpy.hasFocus = false;
await Commands.execute("workbench.action.chat.open");
await flush();
check("Open Chat without focus reveals the pane", groupOfZone("right")?.api.isVisible === true);
check("Open Chat without focus sets the Secondary Side Bar key", contextKeys.getValue("auxiliaryBarVisible") === true);
check("Open Chat focuses the composer", firstSpy.focus >= 1);

// New Chat reuses an empty chat; a busy chat means a new tab.
const panelsBefore = dock.panels.length;
firstSpy.empty = true;
const focusBefore = firstSpy.focus;
await Commands.execute("workbench.action.chat.new");
await flush();
check("New Chat reuses an empty chat instead of opening a tab", dock.panels.length === panelsBefore);
check("the reused chat takes focus", firstSpy.focus === focusBefore + 1);
firstSpy.empty = false;
await Commands.execute("workbench.action.chat.newTab");
await flush();
const agents = dock.panels.filter((panel) => panel.api.component === "agent");
check("with no empty chat, New Chat Tab opens a second one", agents.length === 2);
const second = agents.find((panel) => panel !== first);
check("the new tab lands in the same right group", second?.group.id === first.group.id);

// The tab cycle.
const secondPanel = realPanelOf(second);
await flush();
spyOn(secondPanel, { hasFocus: false, empty: false });
second.api.setActive();
await Commands.execute("workbench.action.chat.nextTab");
check("Next Chat Tab wraps from the last tab to the first", dock.activePanel?.id === first.id);
await Commands.execute("workbench.action.chat.previousTab");
check("Previous Chat Tab wraps back from the first to the last", dock.activePanel?.id === second.id);
await Commands.execute("workbench.action.chat.previousTab");
check("Previous Chat Tab steps back one", dock.activePanel?.id === first.id);

// The close rows.
await Commands.execute("workbench.action.chat.closeTab", { panelId: second.id });
await flush();
check("Close Tab closes the named chat", dock.getPanel(second.id) === undefined);
check("closing one of two chats leaves the pane showing", groupOfZone("right")?.api.isVisible === true);
const third = openInZone("agent", { instance: "three" });
await flush();
await Commands.execute("workbench.action.chat.closeOtherTabs", { panelId: third.id });
await flush();
check("Close Other Tabs closes the group's other chats", dock.getPanel(first.id) === undefined && dock.getPanel(third.id) !== undefined);
const fourth = openInZone("agent", { instance: "four" });
await flush();
await Commands.execute("workbench.action.chat.closeAllTabs", { panelId: fourth.id });
await flush();
check(
  "Close All Tabs closes every chat in the group",
  dock.panels.filter((panel) => panel.api.component === "agent").length === 0,
);
check(
  "closing the last chat hides the right zone, which stays alive and empty",
  groupOfZone("right") !== undefined &&
    groupOfZone("right").panels.length === 0 &&
    groupOfZone("right").api.isVisible === false &&
    contextKeys.getValue("auxiliaryBarVisible") === false,
);

// With no chat open, Open Chat opens one, reveals the zone, and focuses its composer.
await Commands.execute("workbench.action.chat.open");
await flush();
const reopened = dock.panels.filter((panel) => panel.api.component === "agent");
check("Open Chat with no chat opens one", reopened.length === 1);
check("the new chat's zone is showing", groupOfZone("right")?.api.isVisible === true);

if (failures.length > 0) {
  console.error(`agent-keybindings: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-keybindings: all assertions passed");
process.exit(0);
