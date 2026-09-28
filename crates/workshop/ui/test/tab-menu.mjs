// Integration test for the generic tab and its registry-driven menu: the
// tab renderer (src/parts/layout/panel-tab.ts, dispatched as "panel-tab"
// through panel-types.ts), the menu widget's context overlay
// (src/parts/menu/menu.ts), the EditorTitleContext rows the editor and
// stub contributions register, and the confirm-then-close path the X and
// the Close row share (src/parts/layout/panel-close.ts, registered by
// layout.contribution.ts). Mounts a real
// Dockview dock in jsdom with synthetic panel types - "doc" builds a real
// EditorPanel over a stub surface and stub file I/O, "agent", "probe" and
// "side" are bare WorkshopParts, and "pinned" registers closable: false -
// and adds test-only rows whose when, toggled and keybinding rules read
// activeEditor, while the global activeEditor key stays empty.
// Covers: the menu opens at the pointer with the registry rows in order;
// the Close row is enabled through the overlay and shows Ctrl+F4; the stub
// rows render disabled under their final names and shortcuts, the Move row
// under VS Code's tab-menu title while the menubar keeps its own; a row gated
// on activeEditor == 'agent' shows only on agent tabs, and the overlay
// also drives toggled rows, keybinding labels and a submenu's flyout;
// Close acts on the clicked tab rather than the active one, and Close
// Others on the clicked tab's group; a non-closable tab has no X and opens
// no menu; the X on an unsaved editor prompts, and Cancel keeps the tab;
// the X on another group's background tab closes it without activating
// it or its group, and every X is out of the tab order;
// Delete and Backspace on a focused tab leave a non-closable tab open,
// prompt on an unsaved editor (Cancel keeps it), and close a clean tab,
// moving focus to the tab now at its index so a second Delete closes that.
// Run: node --test test/tab-menu.mjs
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
      import "./src/parts/layout/layout.contribution.ts";
      import "./src/parts/menu/stubs.contribution.ts";
      export { createDockview, themeDark } from "dockview";
      export { registerPanelFactory, registerPanelType, resolvePanelContent } from "@workshop/platform/panel-registry";
      export { WorkshopPart } from "@workshop/platform/workshop-part";
      export { Commands } from "@workshop/platform/command-registry";
      export { MenuId, Menus } from "@workshop/platform/menu-registry";
      export { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
      export { CONTEXT_KEY_SERVICE } from "@workshop/platform/context-key-service";
      export { detectPlatform, formatKeybinding, parseKeybinding } from "@workshop/platform/keybinding-parser";
      export { getService } from "@workshop/platform/service-registry";
      export { initZones, openInZone, resetZones } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent, PANEL_TAB } from "./src/parts/layout/panel-types.ts";
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
  throw new Error(`unexpected fetch in the tab-menu test: ${url}`);
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
  "MouseEvent",
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

const bundlePath = path.join(os.tmpdir(), "promptforge-tab-menu-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  registerPanelFactory,
  registerPanelType,
  resolvePanelContent,
  WorkshopPart,
  Commands,
  MenuId,
  Menus,
  KeybindingsRegistry,
  CONTEXT_KEY_SERVICE,
  detectPlatform,
  formatKeybinding,
  parseKeybinding,
  getService,
  initZones,
  openInZone,
  resetZones,
  createPanelComponent,
  createPanelTabComponent,
  PANEL_TAB,
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

const readDoc = async (filePath) => ({ path: filePath, size: 0, token: "t1", text: "" });
const writeDoc = async (filePath, text) => ({ path: filePath, size: text.length, token: "t2", text });

class ProbePart extends WorkshopPart {
  create() {}
}

// One lazy "feature directory" serving every synthetic type. Everything
// but "side" shares the main zone's group.
const feature = {
  register() {
    registerPanelFactory(
      "doc",
      () => new EditorPanel({ createSurface: createStubSurface, readFile: readDoc, writeFile: writeDoc }),
    );
    for (const type of ["agent", "probe", "side", "pinned"]) {
      registerPanelFactory(type, () => new ProbePart());
    }
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
registerPanelType({ type: "agent", title: "Agent", defaultZone: "main", load });
registerPanelType({ type: "probe", title: "Probe", defaultZone: "main", load });
registerPanelType({ type: "side", title: "Side", defaultZone: "right", load });
registerPanelType({ type: "pinned", title: "Pinned", defaultZone: "main", closable: false, load });

// Test-only rows in the tab menu's own group, sorted after every shipped
// row: one gated on the clicked type, one checked by it, one labelled by
// the rule its when selects, and a flyout whose only row is gated on it.
const AGENT_ONLY = "test.tabMenu.agentOnly";
const CHECKED = "test.tabMenu.checked";
const LABELLED = "test.tabMenu.labelled";
const FLYOUT = "test/tabMenu/flyout";
const FLYOUT_ROW = "test.tabMenu.flyoutRow";
Commands.register(AGENT_ONLY, { title: "Agent Only", run() {} });
Commands.register(CHECKED, { title: "Checked", toggled: "activeEditor == 'agent'", run() {} });
Commands.register(LABELLED, { title: "Labelled", run() {} });
Commands.register(FLYOUT_ROW, { title: "Flyout Row", run() {} });
Menus.appendMenuItem(MenuId.EditorTitleContext, {
  command: AGENT_ONLY,
  when: "activeEditor == 'agent'",
  group: "9_test",
  order: 1,
});
Menus.appendMenuItem(MenuId.EditorTitleContext, { submenu: FLYOUT, title: "Flyout", group: "9_test", order: 2 });
Menus.appendMenuItem(MenuId.EditorTitleContext, { command: CHECKED, group: "9_test", order: 3 });
Menus.appendMenuItem(MenuId.EditorTitleContext, { command: LABELLED, group: "9_test", order: 4 });
Menus.appendMenuItem(FLYOUT, { command: FLYOUT_ROW, when: "activeEditor == 'agent'" });
KeybindingsRegistry.registerKeybindingRule({ id: LABELLED, keybinding: "alt+f7", when: "activeEditor == 'doc'" });
KeybindingsRegistry.registerKeybindingRule({ id: LABELLED, keybinding: "alt+f8", when: "activeEditor == 'agent'" });

// The global key names no panel type, so everything the menu reads from
// activeEditor has to come from the overlay.
getService(CONTEXT_KEY_SERVICE).createKey("activeEditor", "").set("");

const platform = detectPlatform();
/** The label the keybinding registry renders for `chord` on this host. */
function chordLabel(chord) {
  const parsed = parseKeybinding(chord, platform);
  return parsed.ok ? formatKeybinding(parsed.value, platform) : `malformed ${chord}`;
}

let dock = null;
function freshDock() {
  const element = window.document.createElement("div");
  element.className = "ws-dock";
  window.document.body.appendChild(element);
  dock = createDockview(element, {
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
  resetZones();
}

async function open(type, params = {}) {
  const panel = openInZone(type, params);
  await flush();
  return panel;
}

const isOpen = (panel) => dock.getPanel(panel.id) === panel;
const partOf = (panel) => resolvePanelContent(panel.view.content);
const tabOf = (panel) => panel.view.tab.element;
const closeButton = (panel) => tabOf(panel).querySelector(".dv-default-tab-action");

function rightClick(panel, x = 40, y = 30) {
  tabOf(panel).dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: x, clientY: y }),
  );
}
const shownPopovers = () =>
  [...window.document.querySelectorAll(".ws-window-titlebar__popover")].filter((popover) => !popover.hidden);
const rootPopover = () =>
  shownPopovers().find((popover) => !popover.classList.contains("ws-window-titlebar__popover--flyout"));
const flyoutPopover = () =>
  shownPopovers().find((popover) => popover.classList.contains("ws-window-titlebar__popover--flyout"));
const rowsOf = (popover) => [...(popover?.querySelectorAll(".ws-window-titlebar__item") ?? [])];
const keysOf = (popover) => rowsOf(popover).map((row) => row.dataset.menuRowKey);
const rowFor = (popover, key) => rowsOf(popover).find((row) => row.dataset.menuRowKey === key);
const labelOf = (row) => row?.querySelector(".ws-window-titlebar__item-label")?.textContent;
const shortcutOf = (row) => row?.querySelector(".ws-window-titlebar__shortcut")?.textContent;
function dismiss() {
  window.document.body.dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true }));
}

const CLOSE = "workbench.action.closeActiveEditor";
const CLOSE_OTHERS = "workbench.action.closeOtherEditors";
const SHIPPED_ROWS = [
  CLOSE,
  CLOSE_OTHERS,
  "workbench.action.closeEditorsToTheRight",
  "workbench.action.closeUnmodifiedEditors",
  "workbench.action.closeAllEditors",
  "workbench.action.reopenWithEditor",
  "workbench.action.keepEditor",
  "workbench.action.pinEditor",
  "workbench.action.moveEditorToNewWindow",
];
// [id, final name, chord or undefined]
const STUBS = [
  ["workbench.action.closeEditorsToTheRight", "Close to the Right", undefined],
  ["workbench.action.closeUnmodifiedEditors", "Close Saved", "ctrlcmd+m u"],
  ["workbench.action.closeAllEditors", "Close All", "ctrlcmd+m w"],
  ["workbench.action.reopenWithEditor", "Reopen Editor With...", undefined],
  ["workbench.action.keepEditor", "Keep Open", "ctrlcmd+m enter"],
  ["workbench.action.pinEditor", "Pin", "ctrlcmd+m shift+enter"],
  ["workbench.action.moveEditorToNewWindow", "Move into New Window", undefined],
];

// --- The menu opens at the pointer with the registry rows ----------------------

freshDock();
const doc = await open("doc", { path: "a.txt" });
const agent = await open("agent");
const probeA = await open("probe", { instance: "a" });
const probeB = await open("probe", { instance: "b" });
const pinned = await open("pinned");
const side = await open("side");
check(
  "the main-zone panels share one group apart from side",
  [agent, probeA, probeB, pinned].every((panel) => panel.group === doc.group) && side.group !== doc.group,
);

rightClick(doc, 40, 30);
const docMenu = rootPopover();
check("right-clicking a closable tab opens one menu", docMenu !== undefined && shownPopovers().length === 1);
check("the menu opens at the pointer", docMenu?.style.left === "40px" && docMenu?.style.top === "30px");
check(
  `the doc tab's menu lists the registry rows in order (got: ${keysOf(docMenu).join(", ")})`,
  keysOf(docMenu).join("|") === [...SHIPPED_ROWS, CHECKED, LABELLED].join("|"),
);
const closeRow = rowFor(docMenu, CLOSE);
check("the Close row reads Close", labelOf(closeRow) === "Close");
check(
  `the Close row shows ${chordLabel("ctrlcmd+f4")} (got: ${shortcutOf(closeRow)})`,
  shortcutOf(closeRow) === chordLabel("ctrlcmd+f4"),
);
check(
  "the Close row is enabled through the overlay while the global activeEditor is empty",
  closeRow?.getAttribute("aria-disabled") === "false",
);
check("the Close Others row reads Close Others", labelOf(rowFor(docMenu, CLOSE_OTHERS)) === "Close Others");
for (const [id, title, chord] of STUBS) {
  const row = rowFor(docMenu, id);
  check(`the ${title} stub renders disabled`, row?.getAttribute("aria-disabled") === "true");
  check(`the ${id} stub reads '${title}' (got: '${labelOf(row)}')`, labelOf(row) === title);
  check(
    `the ${title} stub shows ${chord === undefined ? "no shortcut" : chordLabel(chord)} (got: ${shortcutOf(row)})`,
    shortcutOf(row) === (chord === undefined ? undefined : chordLabel(chord)),
  );
}
const menubarMove = Menus.getMenuItems("menubar/view/editorLayout").find(
  (row) => row.command === "workbench.action.moveEditorToNewWindow",
);
check(
  "the menubar's Move row keeps the command title",
  (menubarMove?.title ?? Commands.lookup(menubarMove?.command ?? "")?.title) === "Move Editor into New Window",
);

// --- The overlay: when, toggled, keybinding labels, and submenus ---------------

check("a row gated on an agent tab stays hidden on a doc tab", rowFor(docMenu, AGENT_ONLY) === undefined);
check("a flyout with no visible rows is dropped on a doc tab", rowFor(docMenu, FLYOUT) === undefined);
check("the toggled row is unchecked on a doc tab", rowFor(docMenu, CHECKED)?.getAttribute("aria-checked") === "false");
check(
  `the doc tab labels the row with its doc rule (got: ${shortcutOf(rowFor(docMenu, LABELLED))})`,
  shortcutOf(rowFor(docMenu, LABELLED)) === chordLabel("alt+f7"),
);
dismiss();
check("an outside pointer closes the tab menu", shownPopovers().length === 0);

rightClick(agent);
const agentMenu = rootPopover();
check("a row gated on activeEditor == 'agent' shows on an agent tab", rowFor(agentMenu, AGENT_ONLY) !== undefined);
check("the toggled row is checked on an agent tab", rowFor(agentMenu, CHECKED)?.getAttribute("aria-checked") === "true");
check(
  `the agent tab labels the row with its agent rule (got: ${shortcutOf(rowFor(agentMenu, LABELLED))})`,
  shortcutOf(rowFor(agentMenu, LABELLED)) === chordLabel("alt+f8"),
);
const flyoutRow = rowFor(agentMenu, FLYOUT);
check("the flyout row shows on an agent tab", flyoutRow !== undefined);
flyoutRow?.click();
check(
  "the overlay reaches the flyout's rows",
  keysOf(flyoutPopover()).join("|") === FLYOUT_ROW,
);
dismiss();
check("an outside pointer closes the menu and its flyout", shownPopovers().length === 0);

// --- Close acts on the clicked tab; Close Others on the clicked tab's group ----

probeB.api.setActive();
rightClick(probeA);
rowFor(rootPopover(), CLOSE)?.click();
await flush();
check("Close closes the clicked tab", !isOpen(probeA));
check("Close leaves the active panel open", isOpen(probeB));
check("running a row closes the menu", shownPopovers().length === 0);

side.api.setActive();
rightClick(probeB);
rowFor(rootPopover(), CLOSE_OTHERS)?.click();
await flush();
check("Close Others keeps the clicked tab", isOpen(probeB));
check("Close Others closes the clicked tab's closable siblings", !isOpen(doc) && !isOpen(agent));
check("Close Others spares the non-closable sibling", isOpen(pinned));
check("Close Others leaves the active panel's other group alone", isOpen(side));

// --- A non-closable tab: no X and no menu ---------------------------------------

check("a non-closable tab has no close button", closeButton(pinned) === null);
rightClick(pinned);
check("right-clicking a non-closable tab opens no menu", shownPopovers().length === 0);

// --- The X: the confirm-then-close path -----------------------------------------

const button = closeButton(probeB);
check(
  "a closable tab's X is the default chip's labelled close action",
  button?.classList.contains("dv-default-tab-action") === true && button?.getAttribute("aria-label") === "Close",
);
const unsaved = await open("doc", { path: "unsaved.txt" });
surfaces.get("unsaved.txt").type("draft\n");
probeB.api.setActive();
closeButton(unsaved)?.click();
await flush();
const prompts = () => partOf(unsaved).element.querySelectorAll(".ws-editor-close-overlay").length;
check("the X on an unsaved editor prompts instead of closing", isOpen(unsaved) && prompts() === 1);
const cancel = [...partOf(unsaved).element.querySelectorAll(".ws-editor-close__button")].find(
  (candidate) => candidate.textContent === "Cancel",
);
check("the prompt offers Cancel", cancel !== undefined);
cancel?.click();
await flush();
check("Cancel keeps the unsaved editor's tab", isOpen(unsaved) && partOf(unsaved).isDirty() === true);

closeButton(probeB)?.click();
await flush();
check("the X closes a clean panel", !isOpen(probeB));

// --- The X never activates its tab ----------------------------------------------

const sideFront = await open("side", { instance: "front" });
check(
  "the side zone holds a background tab behind its active one",
  side.group === sideFront.group && side.group.activePanel === sideFront,
);
unsaved.api.setActive();
await flush();
const closeX = closeButton(side);
closeX?.dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
closeX?.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
await flush();
check("the X on another group's background tab closes it", !isOpen(side));
check("the X on another group's tab leaves the active group active", dock.activeGroup === unsaved.group);
check("the X on another group's tab leaves the active panel active", dock.activePanel === unsaved);
const dockButtons = [...window.document.querySelectorAll(".ws-dock .dv-default-tab-action")];
check(
  "every close button in the dock is out of the tab order",
  dockButtons.length > 0 && dockButtons.every((candidate) => candidate.tabIndex === -1),
);

// --- Delete and Backspace on a focused tab: the X's path ------------------------

/** Presses `key` on the panel's focused tab: the Dockview wrapper its tab strip's own key handler matches. */
function pressOnTab(panel, key) {
  const wrapper = tabOf(panel).parentElement;
  wrapper?.focus();
  wrapper?.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}
function cancelPrompt() {
  [...partOf(unsaved).element.querySelectorAll(".ws-editor-close__button")]
    .find((candidate) => candidate.textContent === "Cancel")
    ?.click();
}

for (const key of ["Delete", "Backspace"]) {
  pressOnTab(pinned, key);
  await flush();
  check(`${key} on a focused non-closable tab leaves it open`, isOpen(pinned));

  pressOnTab(unsaved, key);
  await flush();
  check(`${key} on a focused unsaved editor tab prompts instead of closing`, isOpen(unsaved) && prompts() === 1);
  cancelPrompt();
  await flush();
  check(
    `Cancel after ${key} keeps the unsaved editor's tab`,
    isOpen(unsaved) && prompts() === 0 && partOf(unsaved).isDirty() === true,
  );

  const clean = await open("probe", { instance: key });
  pressOnTab(clean, key);
  await flush();
  check(`${key} on a focused clean closable tab closes it`, !isOpen(clean));
}

const left = await open("probe", { instance: "left" });
const middle = await open("probe", { instance: "middle" });
const right = await open("probe", { instance: "right" });
const middleIndex = middle.group.panels.indexOf(middle);
check(
  "three clean tabs sit side by side in one group",
  [left, right].every((panel) => panel.group === middle.group) &&
    left.group.panels.indexOf(left) === middleIndex - 1 &&
    right.group.panels.indexOf(right) === middleIndex + 1,
);
pressOnTab(middle, "Delete");
await flush();
check("Delete on the middle tab closes it", !isOpen(middle));
check(
  "Delete moves focus to the wrapper of the tab now at the closed tab's index",
  right.group.panels[middleIndex] === right && window.document.activeElement === tabOf(right).parentElement,
);
window.document.activeElement?.dispatchEvent(
  new window.KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }),
);
await flush();
check("a second Delete closes the tab focus moved to", !isOpen(right) && isOpen(left));
check(
  "Delete on the last tab moves focus to the wrapper of the tab before it",
  window.document.activeElement === tabOf(left).parentElement,
);

if (failures.length > 0) {
  console.error(`tab-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("tab-menu: all assertions passed");
process.exit(0);
