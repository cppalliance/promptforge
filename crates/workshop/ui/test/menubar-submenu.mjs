// Unit test for the menu popover widget (src/parts/menu/menu.ts): one
// popover rebuilt at every open from the menu registry's getMenuItems,
// command rows versus submenu rows, a single child-submenu slot opened
// on hover (after 250ms) or ArrowRight (at once) and closed on ArrowLeft or
// 750ms after the pointer moves to another row (recursive for nested
// flyouts), rows firing on mouseup (press, drag, release) with the click a
// press ends ignored, flyouts and context menus flipping to stay on
// screen, a max height that leaves the menu room to scroll under a 7px
// bar, group-boundary separators, empty submenus dropped, rows
// hidden by a failing when, aria-disabled by a failing precondition,
// menuitemcheckbox with aria-checked from toggled, shortcut labels from
// the keybinding registry, the context value passed as the first run
// argument, self-owned dismissal (Escape, outside pointer, window blur),
// and rebuild-while-open only when a referenced context key changes with
// focus restored by row key. Bundles the module with esbuild and drives
// it against jsdom.
// Run: node --test test/menubar-submenu.mjs
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
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { Menu } from "./src/parts/menu/menu.ts";
      export { CommandRegistry } from "@workshop/platform/command-registry";
      export { MenuRegistry } from "@workshop/platform/menu-registry";
      export { ContextKeyService } from "@workshop/platform/context-key-service";
      export { createKeybindingsRegistry } from "@workshop/platform/keybinding-registry";
      export { registerService } from "@workshop/platform/service-registry";
      export { STATUS_BAR } from "@workshop/platform/status-bar";
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
const { Menu, CommandRegistry, MenuRegistry, ContextKeyService, createKeybindingsRegistry, registerService, STATUS_BAR } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Fixture: registries, keys, and a menu tree with two flyout levels -------

const commands = new CommandRegistry();
const menus = new MenuRegistry();
const contextKeys = new ContextKeyService();
const keybindings = createKeybindingsRegistry("linux");

const canOpen = contextKeys.createKey("canOpen", false);
const wordWrap = contextKeys.createKey("wordWrap", true);
const unrelated = contextKeys.createKey("unrelated", false);

const runs = [];
commands.register("file.new", { title: "New File", run: (...args) => runs.push(["file.new", ...args]) });
commands.register("file.open", { title: "Open", precondition: "canOpen", run: (...args) => runs.push(["file.open", ...args]) });
commands.register("view.wordWrap", { title: "Word Wrap", toggled: "wordWrap", run: () => runs.push(["view.wordWrap"]) });
commands.register("file.reopen", { title: "Reopen Closed Editor", run: () => runs.push(["file.reopen"]) });
commands.register("file.deepAction", { title: "Deep Action", run: () => runs.push(["file.deepAction"]) });
keybindings.registerKeybindingRule({ id: "file.new", keybinding: "ctrl+n" });

menus.appendMenuItem("menubar/file", { command: "file.new" });
menus.appendMenuItem("menubar/file", { command: "file.open" });
menus.appendMenuItem("menubar/file", { command: "file.hidden", title: "Hidden", when: "canOpen" });
menus.appendMenuItem("menubar/file", { submenu: "menubar/file/recent", title: "Recent", group: "1_recent" });
menus.appendMenuItem("menubar/file", { submenu: "menubar/file/empty", title: "Empty", group: "1_recent" });
menus.appendMenuItem("menubar/file", { command: "view.wordWrap", group: "2_view" });
menus.appendMenuItem("menubar/file/recent", { command: "file.reopen" });
menus.appendMenuItem("menubar/file/recent", { submenu: "menubar/file/recent/deep", title: "Deep", group: "1_deep" });
menus.appendMenuItem("menubar/file/recent/deep", { command: "file.deepAction" });
// The empty submenu has no rows at all; the widget drops its row.

const anchor = window.document.createElement("button");
anchor.type = "button";
window.document.body.appendChild(anchor);

const menu = new Menu({ menus, commands, contextKeys, keybindings });

function popovers() {
  return [...window.document.querySelectorAll(".ws-window-titlebar__popover")].filter((el) => !el.hidden);
}
function rowsOf(popover) {
  return [...popover.querySelectorAll(":scope > .ws-window-titlebar__item")];
}
function rowByLabel(popover, label) {
  return rowsOf(popover).find((row) => row.querySelector(".ws-window-titlebar__item-label")?.textContent === label);
}
function pressKey(key) {
  window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}
function pointerDownOn(target) {
  target.dispatchEvent(new window.Event("pointerdown", { bubbles: true }));
}
function hover(target) {
  target.dispatchEvent(new window.Event("pointerenter", { bubbles: false }));
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function mouse(target, type, init = {}) {
  return target.dispatchEvent(
    new window.MouseEvent(type, { bubbles: true, cancelable: true, button: 0, detail: 1, ...init }),
  );
}

// --- Open renders command and submenu rows, separators, shortcuts -------------

menu.open("menubar/file", anchor);

{
  check("one popover is shown", popovers().length === 1);
  const popover = popovers()[0];
  check("the popover has role=menu", popover?.getAttribute("role") === "menu");
  const labels = rowsOf(popover).map((row) => row.querySelector(".ws-window-titlebar__item-label")?.textContent);
  check(
    "rows render in sort order with the command title as the default label",
    labels.join(",") === "New File,Open,Recent,Word Wrap",
  );
  check("every row is a type=button menuitem", rowsOf(popover).every((row) => row.type === "button"));
  check(
    "a when that fails hides the row",
    rowByLabel(popover, "Hidden") === undefined,
  );
  check(
    "an empty submenu is dropped",
    rowByLabel(popover, "Empty") === undefined,
  );
  check(
    "a group boundary renders a separator",
    popover.querySelectorAll(".ws-window-titlebar__separator").length === 2,
  );
  const newRow = rowByLabel(popover, "New File");
  check(
    "the shortcut label comes from the keybinding registry",
    newRow?.querySelector(".ws-window-titlebar__shortcut")?.textContent === "Ctrl+N",
  );
  const recentRow = rowByLabel(popover, "Recent");
  check(
    "a submenu row has aria-haspopup and a collapsed state",
    recentRow?.getAttribute("aria-haspopup") === "menu" && recentRow?.getAttribute("aria-expanded") === "false",
  );
}

// --- Disabled and checked rendering -------------------------------------------

{
  const popover = popovers()[0];
  const openRow = rowByLabel(popover, "Open");
  check("a failing precondition renders aria-disabled", openRow?.getAttribute("aria-disabled") === "true");
  openRow?.dispatchEvent(new window.Event("click", { bubbles: true }));
  check("clicking a disabled row runs nothing", runs.length === 0);

  const wrapRow = rowByLabel(popover, "Word Wrap");
  check(
    "a toggled row renders as a checked menuitemcheckbox",
    wrapRow?.getAttribute("role") === "menuitemcheckbox" && wrapRow?.getAttribute("aria-checked") === "true",
  );

  wordWrap.set(false);
  const wrapRowAfter = rowByLabel(popovers()[0], "Word Wrap");
  check(
    "a toggled-key change rebuilds the open menu unchecked",
    wrapRowAfter?.getAttribute("aria-checked") === "false",
  );

  canOpen.set(true);
  const after = popovers()[0];
  check(
    "a precondition-key change re-enables the row and reveals the gated row",
    rowByLabel(after, "Open")?.getAttribute("aria-disabled") === "false" && rowByLabel(after, "Hidden") !== undefined,
  );
}

// --- Rebuild while open restores focus by row key; unrelated keys do not ------

{
  const popover = popovers()[0];
  const openRow = rowByLabel(popover, "Open");
  openRow.focus();
  check("the row holds focus", window.document.activeElement === openRow);
  const before = rowByLabel(popovers()[0], "Open");
  unrelated.set(true);
  check(
    "an unrelated key change does not rebuild",
    rowByLabel(popovers()[0], "Open") === before && window.document.activeElement === before,
  );
  wordWrap.set(true);
  const rebuilt = rowByLabel(popovers()[0], "Open");
  check(
    "a referenced key change rebuilds and restores focus by row key",
    rebuilt !== before && window.document.activeElement === rebuilt,
  );
}

// --- Flyout: opens 250ms after hover, closes 750ms after the pointer leaves ----

{
  const popover = popovers()[0];
  hover(rowByLabel(popover, "Recent"));
  check("hovering a submenu row does not open its flyout at once", popovers().length === 1);
  await sleep(100);
  check("the flyout is still closed before the 250ms show delay", popovers().length === 1);
  await sleep(250);
  check("hovering a submenu row opens its flyout after 250ms", popovers().length === 2);
  check(
    "the parent row reports the flyout expanded",
    rowByLabel(popover, "Recent")?.getAttribute("aria-expanded") === "true",
  );
  const flyout = popovers()[1];
  check(
    "the flyout renders the child menu's rows",
    rowByLabel(flyout, "Reopen Closed Editor") !== undefined && rowByLabel(flyout, "Deep") !== undefined,
  );
  check(
    "a hover-opened flyout leaves the keyboard focus where it was",
    window.document.activeElement !== rowByLabel(flyout, "Reopen Closed Editor"),
  );

  hover(rowByLabel(popover, "New File"));
  check("hovering a command row leaves the flyout open at first", popovers().length === 2);
  await sleep(400);
  check("the flyout survives well inside the 750ms hide delay", popovers().length === 2);
  await sleep(450);
  check("the flyout closes 750ms after the pointer moved to a command row", popovers().length === 1);
  check(
    "the parent row reports the flyout collapsed",
    rowByLabel(popover, "Recent")?.getAttribute("aria-expanded") === "false",
  );
}

// --- Leaving a submenu row before 250ms cancels its open ------------------------

{
  const popover = popovers()[0];
  hover(rowByLabel(popover, "Recent"));
  await sleep(100);
  hover(rowByLabel(popover, "New File"));
  await sleep(300);
  check("moving off a submenu row inside the show delay opens nothing", popovers().length === 1);
}

// --- Entering the flyout cancels the pending hide -------------------------------

{
  const popover = popovers()[0];
  hover(rowByLabel(popover, "Recent"));
  await sleep(300);
  const flyout = popovers()[1];
  hover(rowByLabel(popover, "New File"));
  await sleep(300);
  hover(flyout);
  await sleep(700);
  check("the pointer reaching the flyout inside the hide delay keeps it open", popovers().length === 2);
  hover(rowByLabel(popover, "Word Wrap"));
  await sleep(900);
  check("leaving to a command row afterwards closes it after the delay", popovers().length === 1);
}

// --- Click on a submenu row opens at once; a sibling submenu replaces it after the delay ---

{
  const second = menus.appendMenuItem("menubar/file", { submenu: "menubar/file/other", title: "Other", group: "1_recent" });
  const otherRow = menus.appendMenuItem("menubar/file/other", { command: "file.otherAction", title: "Other Action" });
  commands.register("file.otherAction", { title: "Other Action", run: () => runs.push(["file.otherAction"]) });
  menu.close();
  menu.open("menubar/file", anchor);
  const popover = popovers()[0];
  rowByLabel(popover, "Recent").click();
  check("clicking a submenu row opens its flyout immediately", popovers().length === 2);
  hover(rowByLabel(popover, "Other"));
  check("hovering a sibling submenu row keeps the first flyout until the delay", rowByLabel(popovers()[1], "Reopen Closed Editor") !== undefined);
  await sleep(350);
  check(
    "after the delay the sibling's flyout replaces it",
    popovers().length === 2 && rowByLabel(popovers()[1], "Other Action") !== undefined,
  );
  menu.close();
  second.dispose();
  otherRow.dispose();
  menu.open("menubar/file", anchor);
}

// --- ArrowRight opens, ArrowLeft closes, nested flyouts recurse -----------------

{
  const popover = popovers()[0];
  rowByLabel(popover, "Recent").focus();
  pressKey("ArrowRight");
  check("ArrowRight on a submenu row opens the flyout", popovers().length === 2);
  const flyout = popovers()[1];
  check(
    "ArrowRight moves focus into the flyout's first row",
    window.document.activeElement === rowByLabel(flyout, "Reopen Closed Editor"),
  );

  pressKey("ArrowDown");
  check("ArrowDown moves focus within the flyout", window.document.activeElement === rowByLabel(flyout, "Deep"));
  pressKey("ArrowRight");
  check("ArrowRight opens a nested flyout", popovers().length === 3);
  check(
    "the nested flyout renders its rows",
    rowByLabel(popovers()[2], "Deep Action") !== undefined,
  );

  pressKey("ArrowLeft");
  check("ArrowLeft closes the nested flyout", popovers().length === 2);
  check(
    "ArrowLeft returns focus to the parent flyout row",
    window.document.activeElement === rowByLabel(popovers()[1], "Deep"),
  );
  pressKey("ArrowLeft");
  check("ArrowLeft closes the flyout", popovers().length === 1);
  check(
    "focus returns to the parent submenu row",
    window.document.activeElement === rowByLabel(popovers()[0], "Recent"),
  );
}

// --- Activation passes the context value and closes ----------------------------

menu.close();
runs.length = 0;
menu.open("menubar/file", anchor, { path: "src/a.ts" });

{
  const popover = popovers()[0];
  rowByLabel(popover, "New File").dispatchEvent(new window.Event("click", { bubbles: true }));
  check("activating a row closes the menu", popovers().length === 0);
  check(
    "the context value is the run's first argument",
    runs.length === 1 && runs[0][0] === "file.new" && runs[0][1]?.path === "src/a.ts",
  );
}

// --- Rows fire on mouseup: press, drag, release ---------------------------------

menu.close();
runs.length = 0;
menu.open("menubar/file", anchor);

{
  const row = rowByLabel(popovers()[0], "New File");
  mouse(row, "mousedown");
  check("pressing a row fires nothing yet", runs.length === 0 && popovers().length === 1);
  mouse(row, "mouseup");
  check(
    "releasing on the row fires its command and closes the menu",
    runs.length === 1 && runs[0][0] === "file.new" && popovers().length === 0,
  );
  mouse(row, "click");
  check("the click that ends a press does not fire the command again", runs.length === 1);
}

menu.open("menubar/file", anchor);
{
  mouse(anchor, "mousedown");
  mouse(rowByLabel(popovers()[0], "New File"), "mouseup");
  check("press on the anchor, drag onto a row, release fires that row", runs.length === 2 && popovers().length === 0);
}

canOpen.set(false);
menu.open("menubar/file", anchor);
{
  const popover = popovers()[0];
  mouse(rowByLabel(popover, "New File"), "mouseup", { button: 2 });
  check("a non-primary release fires nothing", runs.length === 2 && popovers().length === 1);
  mouse(rowByLabel(popover, "Open"), "mouseup");
  check("releasing on a disabled row runs nothing and keeps the menu open", runs.length === 2 && popovers().length === 1);
  mouse(popover, "mouseup");
  check("releasing on the popover's dead space runs nothing", runs.length === 2 && popovers().length === 1);
  rowByLabel(popover, "New File").click();
  check("a keyboard-style click (detail 0) still fires", runs.length === 3);
}

// --- Flipping and the max height --------------------------------------------------

{
  const rects = new WeakMap();
  const rect = (left, top, width, height) => ({
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON() {},
  });
  const originalRect = window.HTMLElement.prototype.getBoundingClientRect;
  window.HTMLElement.prototype.getBoundingClientRect = function () {
    if (rects.has(this)) return rects.get(this);
    if (this.classList.contains("ws-window-titlebar__popover")) return rect(0, 0, 200, 300);
    return originalRect.call(this);
  };
  window.innerWidth = 1000;
  window.innerHeight = 800;
  const at = (popover) => [popover.style.left, popover.style.top, popover.style.maxHeight].join(" ");

  menu.close();
  menu.open("menubar/file", { x: 100, y: 100 });
  check("a context menu with room opens at the pointer", at(popovers()[0]) === "100px 100px 665px");
  check(
    "a context menu is marked as one, for its fade",
    popovers()[0].classList.contains("ws-window-titlebar__popover--context"),
  );
  menu.close();
  menu.open("menubar/file", { x: 900, y: 700 });
  check(
    "a context menu flips left and up when it would leave the window",
    at(popovers()[0]) === "700px 400px 365px",
  );
  menu.close();

  rects.set(anchor, rect(950, 0, 40, 35));
  menu.open("menubar/file", anchor);
  check("a dropdown from the bar slides left to stay on screen", at(popovers()[0]) === "800px 35px 730px");
  check(
    "a dropdown has no fade and a menu scrollbar",
    !popovers()[0].classList.contains("ws-window-titlebar__popover--context") &&
      popovers()[0].classList.contains("scrollbar-menu"),
  );
  menu.close();

  rects.set(anchor, rect(100, 0, 40, 35));
  menu.open("menubar/file", anchor);
  check("a dropdown with room opens under its button", at(popovers()[0]) === "100px 35px 730px");
  const recentRow = rowByLabel(popovers()[0], "Recent");
  rects.set(recentRow, rect(100, 120, 200, 22));
  recentRow.click();
  check("a flyout with room opens beside its parent row", at(popovers()[1]) === "300px 120px 645px");
  menu.close();

  menu.open("menubar/file", anchor);
  const crowdedRow = rowByLabel(popovers()[0], "Recent");
  rects.set(crowdedRow, rect(700, 120, 200, 22));
  crowdedRow.click();
  check("a flyout flips to the parent row's left side when the right has no room", at(popovers()[1]) === "500px 120px 645px");
  menu.close();

  menu.open("menubar/file", anchor);
  const lowRow = rowByLabel(popovers()[0], "Recent");
  rects.set(lowRow, rect(100, 700, 200, 22));
  lowRow.click();
  check("a flyout near the bottom shifts up to fit", at(popovers()[1]) === "300px 500px 265px");
  menu.close();

  window.HTMLElement.prototype.getBoundingClientRect = originalRect;
}

// --- A failed command reports to the status bar --------------------------------

const statusMessages = [];
const statusRegistration = registerService(STATUS_BAR, () => ({
  showLocal: (label, severity) => statusMessages.push([label, severity]),
}));
commands.register("file.boom", { title: "Boom", run: () => Promise.reject(new Error("kaboom")) });
menus.appendMenuItem("menubar/file", { command: "file.boom", group: "9_z" });

menu.open("menubar/file", anchor);
{
  const popover = popovers()[0];
  rowByLabel(popover, "Boom").dispatchEvent(new window.Event("click", { bubbles: true }));
  await new Promise((resolve) => setTimeout(resolve, 0));
  check("activating a failing row closes the menu", popovers().length === 0);
  check(
    "a failed command posts an error to the status bar",
    statusMessages.length === 1 &&
      statusMessages[0][1] === "error" &&
      statusMessages[0][0].includes("file.boom") &&
      statusMessages[0][0].includes("kaboom"),
  );
}
statusRegistration.dispose();

// --- Dismissal: Escape, outside pointer, window blur ----------------------------

menu.open("menubar/file", anchor);
pressKey("Escape");
check("Escape closes the menu", popovers().length === 0);
check("Escape returns focus to the anchor", window.document.activeElement === anchor);

menu.open("menubar/file", anchor);
pointerDownOn(window.document.body);
check("an outside pointer press closes the menu", popovers().length === 0);

menu.open("menubar/file", anchor);
pointerDownOn(popovers()[0]);
check("an inside pointer press keeps the menu open", popovers().length === 1);
window.dispatchEvent(new window.Event("blur"));
check("window blur closes the menu", popovers().length === 0);

menu.open("menubar/file", anchor);
menu.dispose();
check("dispose closes an open menu and removes its popover", window.document.querySelector(".ws-window-titlebar__popover") === null);

if (failures.length > 0) {
  console.error(`menubar-submenu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("menubar-submenu: all assertions passed");
