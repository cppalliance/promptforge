// Unit test for the menu popover widget (src/parts/menu/menu.ts): one
// popover rebuilt at every open from the menu registry's getMenuItems,
// command rows versus submenu rows, a single child-submenu slot opened
// on hover or ArrowRight and closed on ArrowLeft (recursive for nested
// flyouts), group-boundary separators, empty submenus dropped, rows
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
      export { CommandRegistry } from "./src/services/command-registry.ts";
      export { MenuRegistry } from "./src/services/menu-registry.ts";
      export { ContextKeyService } from "./src/services/context-key-service.ts";
      export { createKeybindingsRegistry } from "./src/services/keybinding-registry.ts";
      export { registerService } from "./src/services/service-registry.ts";
      export { STATUS_BAR } from "./src/services/status-bar.ts";
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

// --- Flyout open on hover, close on sibling hover ------------------------------

{
  const popover = popovers()[0];
  hover(rowByLabel(popover, "Recent"));
  check("hovering a submenu row opens its flyout", popovers().length === 2);
  check(
    "the parent row reports the flyout expanded",
    rowByLabel(popover, "Recent")?.getAttribute("aria-expanded") === "true",
  );
  const flyout = popovers()[1];
  check(
    "the flyout renders the child menu's rows",
    rowByLabel(flyout, "Reopen Closed Editor") !== undefined && rowByLabel(flyout, "Deep") !== undefined,
  );
  hover(rowByLabel(popover, "New File"));
  check("hovering a command row closes the flyout", popovers().length === 1);
  check(
    "the parent row reports the flyout collapsed",
    rowByLabel(popover, "Recent")?.getAttribute("aria-expanded") === "false",
  );
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
