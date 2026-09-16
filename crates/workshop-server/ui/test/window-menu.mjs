// Unit test for the menubar (src/ui/menu/menubar.ts) composing the menu
// popover widget (src/ui/menu/menu.ts) over the services registries, and
// for the legacy composition root (src/ui/menu/window-menu.ts), which
// fills the shipped empty nav through the menubar's button generator
// until the composition-root step retires it. Bundles the TS modules
// with esbuild - with "@tauri-apps/api/window" aliased to the recording
// stub in test/helpers - and drives them against jsdom built from the
// real index.html. Covers: button generation in registry sort order,
// click toggle, one-menu-at-a-time, rollover, ArrowLeft/Right between
// menus with flyout navigation yielding to the widget, Escape, outside
// pointer and blur dismissal, disabled rows and context-key rebuilds,
// command dispatch with shortcut hints, and disposal; then the legacy
// setupWindowMenus over the generated nav: the five buttons, New Agent
// dispatch, Edit target preservation, the Model menu's catalog rows,
// the About dialog's focus trap, and teardown.
// Run: node --test test/window-menu.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

async function bundle(contents) {
  const result = await esbuild.build({
    stdin: { contents, resolveDir: path.join(uiDir, ".."), loader: "ts" },
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    logLevel: "silent",
    // The modules under test import their colocated CSS; strip it - the
    // test drives only the JS, and jsdom applies no stylesheets anyway.
    loader: { ".css": "empty" },
    // Stands in for the build-time crate-version define (build.mjs and
    // the crate's build.rs): the About dialog must render this value.
    define: { __APP_VERSION__: JSON.stringify("0.0.0-test") },
    alias: {
      "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
    },
  });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}`);
}

const { Menubar, CommandRegistry, MenuRegistry, MenuId, ContextKeyService, createKeybindingsRegistry } =
  await bundle(`
    export { Menubar } from "./src/ui/menu/menubar.ts";
    export { CommandRegistry } from "./src/services/command-registry.ts";
    export { MenuRegistry, MenuId } from "./src/services/menu-registry.ts";
    export { ContextKeyService } from "./src/services/context-key-service.ts";
    export { createKeybindingsRegistry } from "./src/services/keybinding-registry.ts";
  `);
const { setupWindowMenus } = await bundle(`export { setupWindowMenus } from "./src/ui/menu/window-menu.ts";`);
const { ModelService } = await bundle(`export { ModelService } from "./src/services/model-service.ts";`);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function installGlobals(window) {
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.Element = window.Element;
  globalThis.HTMLElement = window.HTMLElement;
  globalThis.HTMLButtonElement = window.HTMLButtonElement;
  globalThis.HTMLInputElement = window.HTMLInputElement;
  globalThis.HTMLTextAreaElement = window.HTMLTextAreaElement;
  globalThis.Node = window.Node;
}

// Each scenario gets a fresh jsdom and fresh registries: the widgets read
// the globals and attach listeners to the DOM they find at call time.
function menubarScenario() {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  installGlobals(window);

  const commands = new CommandRegistry();
  const menus = new MenuRegistry();
  const contextKeys = new ContextKeyService();
  const keybindings = createKeybindingsRegistry("linux");
  const runs = [];

  // Registered out of order on purpose: the bar renders registry sort
  // order, not registration order.
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/view", title: "View", order: 3 });
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/file", title: "File", order: 1 });
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/edit", title: "Edit", order: 2 });

  commands.register("file.new", { title: "New File", run: (...args) => runs.push(["file.new", ...args]) });
  commands.register("file.close", { title: "Close Window", run: () => runs.push(["file.close"]) });
  keybindings.registerKeybindingRule({ id: "file.new", keybinding: "ctrl+n" });
  menus.appendMenuItem("menubar/file", { command: "file.new" });
  menus.appendMenuItem("menubar/file", { command: "file.close", group: "1_close" });

  const canEdit = contextKeys.createKey("canEdit", false);
  commands.register("edit.undo", { title: "Undo", precondition: "canEdit", run: () => runs.push(["edit.undo"]) });
  menus.appendMenuItem("menubar/edit", { command: "edit.undo" });

  menus.appendMenuItem("menubar/view", { submenu: "menubar/view/appearance", title: "Appearance" });
  commands.register("view.toggle", { title: "Toggle Center", run: () => runs.push(["view.toggle"]) });
  menus.appendMenuItem("menubar/view/appearance", { command: "view.toggle" });

  const nav = window.document.querySelector(".ws-window-titlebar__menus");
  const menubar = new Menubar(nav, { menus, commands, contextKeys, keybindings });

  const button = (id) => window.document.querySelector(`.ws-window-titlebar__menu[data-menu="${id}"]`);
  const popovers = () =>
    [...window.document.querySelectorAll(".ws-window-titlebar__popover")].filter((el) => !el.hidden);
  const rowsOf = (popover) => [...popover.querySelectorAll(":scope > .ws-window-titlebar__item")];
  const rowByLabel = (popover, label) =>
    rowsOf(popover).find((row) => row.querySelector(".ws-window-titlebar__item-label")?.textContent === label);
  const keydown = (key) =>
    window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  const hover = (target) => target.dispatchEvent(new window.Event("pointerenter", { bubbles: false }));
  return { window, menubar, runs, canEdit, button, popovers, rowsOf, rowByLabel, keydown, hover };
}

// --- Buttons generate in registry sort order ------------------------------------

{
  const { window, button } = menubarScenario();
  const buttons = [...window.document.querySelectorAll(".ws-window-titlebar__menu")];
  check(
    "the bar generates buttons in registry sort order",
    buttons.map((b) => b.textContent).join(",") === "File,Edit,View",
  );
  check(
    "the buttons carry last-segment data-menu selectors and the popup state",
    buttons.every(
      (b) => b.getAttribute("aria-haspopup") === "menu" && b.getAttribute("aria-expanded") === "false",
    ) && button("file") !== null && button("edit") !== null && button("view") !== null,
  );
}

// --- Click toggles the menu ------------------------------------------------------

{
  const { button, popovers, rowsOf, rowByLabel } = menubarScenario();
  button("file").click();
  check("clicking a button opens its menu", popovers().length === 1);
  check("the open button is announced expanded", button("file").getAttribute("aria-expanded") === "true");
  const popover = popovers()[0];
  check(
    "the rows render in sort order with a group separator",
    rowsOf(popover).map((row) => row.querySelector(".ws-window-titlebar__item-label").textContent).join(",") ===
      "New File,Close Window" && popover.querySelectorAll(".ws-window-titlebar__separator").length === 1,
  );
  check(
    "the shortcut hint comes from the keybinding registry",
    rowByLabel(popover, "New File")?.querySelector(".ws-window-titlebar__shortcut")?.textContent === "Ctrl+N",
  );
  button("file").click();
  check("clicking the open menu's button closes it", popovers().length === 0);
  check("the closed button collapses", button("file").getAttribute("aria-expanded") === "false");
}

// --- One menu at a time -----------------------------------------------------------

{
  const { button, popovers, rowByLabel } = menubarScenario();
  button("file").click();
  button("edit").click();
  check(
    "clicking another button switches the open menu",
    popovers().length === 1 && rowByLabel(popovers()[0], "Undo") !== undefined,
  );
  check(
    "the replaced button collapses",
    button("file").getAttribute("aria-expanded") === "false" &&
      button("edit").getAttribute("aria-expanded") === "true",
  );
}

// --- Rollover ----------------------------------------------------------------------

{
  const { button, popovers, rowsOf, hover } = menubarScenario();
  hover(button("edit"));
  check("hover with no menu open opens nothing", popovers().length === 0);
  button("file").click();
  hover(button("edit"));
  check("hovering another button while open switches the menu", rowByLabelSafe(popovers(), "Undo"));
  const rowsBefore = rowsOf(popovers()[0]);
  hover(button("edit"));
  check(
    "hovering the open menu's own button does not rebuild its rows",
    rowsOf(popovers()[0]).every((row, index) => row === rowsBefore[index]),
  );
}

function rowByLabelSafe(popovers, label) {
  return (
    popovers.length === 1 &&
    [...popovers[0].querySelectorAll(".ws-window-titlebar__item-label")].some((el) => el.textContent === label)
  );
}

// --- Disabled rows and the context-key rebuild ---------------------------------------

{
  const { button, popovers, rowByLabel, runs, canEdit } = menubarScenario();
  button("edit").click();
  const undo = () => rowByLabel(popovers()[0], "Undo");
  check("a failing precondition renders the row disabled", undo()?.getAttribute("aria-disabled") === "true");
  undo().click();
  check("clicking a disabled row runs nothing and keeps the menu open", runs.length === 0 && popovers().length === 1);
  canEdit.set(true);
  check("a referenced key change re-enables the row while open", undo()?.getAttribute("aria-disabled") === "false");
  undo().click();
  check("the enabled row runs its command and closes the menu", runs.join(",") === "edit.undo" && popovers().length === 0);
}

// --- Command dispatch ------------------------------------------------------------------

{
  const { button, popovers, rowByLabel, runs } = menubarScenario();
  button("file").click();
  rowByLabel(popovers()[0], "New File").click();
  check("activating a row runs the command and closes the menu", runs.length === 1 && popovers().length === 0);
  check("a row without args runs with none", runs[0].join(",") === "file.new");
}

// --- ArrowLeft/Right between menus, yielding to flyout navigation -----------------------

{
  const { window, button, popovers, rowsOf, rowByLabel, keydown } = menubarScenario();
  button("file").click();
  keydown("ArrowDown");
  check("ArrowDown focuses the first row", window.document.activeElement === rowByLabel(popovers()[0], "New File"));
  keydown("ArrowRight");
  check("ArrowRight opens the next menu", rowByLabelSafe(popovers(), "Undo"));
  check(
    "ArrowRight focuses the new menu's first row",
    window.document.activeElement === rowByLabel(popovers()[0], "Undo"),
  );
  keydown("ArrowRight");
  check("ArrowRight again opens the third menu", rowByLabelSafe(popovers(), "Appearance"));
  keydown("ArrowDown");
  const appearance = rowByLabel(popovers()[0], "Appearance");
  check("the view menu's row is the Appearance submenu", window.document.activeElement === appearance);
  keydown("ArrowRight");
  check(
    "ArrowRight on a submenu row opens the flyout instead of switching menus",
    popovers().length === 2 && rowByLabel(popovers()[1], "Toggle Center") !== undefined,
  );
  keydown("ArrowLeft");
  check(
    "ArrowLeft with a flyout open closes the flyout instead of switching menus",
    popovers().length === 1 && window.document.activeElement === appearance,
  );
  keydown("ArrowLeft");
  check("ArrowLeft now opens the previous menu", rowByLabelSafe(popovers(), "Undo"));
  keydown("ArrowLeft");
  keydown("ArrowLeft");
  check("ArrowLeft from the first menu wraps to the last", rowByLabelSafe(popovers(), "Appearance"));
  keydown("Escape");
}

// --- Dismissal: Escape, outside pointer, window blur --------------------------------------

{
  const { window, button, popovers, keydown } = menubarScenario();
  button("file").click();
  keydown("Escape");
  check("Escape closes the menu", popovers().length === 0);
  check("Escape returns focus to the menu button", window.document.activeElement === button("file"));

  button("file").click();
  window.document.body.dispatchEvent(new window.Event("pointerdown", { bubbles: true }));
  check("an outside pointer press closes the menu", popovers().length === 0);

  button("file").click();
  window.dispatchEvent(new window.Event("blur"));
  check("window blur closes the menu", popovers().length === 0);
}

// --- Disposal ------------------------------------------------------------------------------

{
  const { window, button, popovers, menubar } = menubarScenario();
  const fileButton = button("file");
  fileButton.click();
  menubar.dispose();
  check("disposal closes the open menu", popovers().length === 0);
  check(
    "disposal removes the generated buttons",
    window.document.querySelectorAll(".ws-window-titlebar__menu").length === 0,
  );
  fileButton.click();
  check("a disposed bar's button no longer opens anything", popovers().length === 0);
}

// --- Legacy bridge: setupWindowMenus over the generated nav ----------------------------------
// Until the composition-root step retires window-menu.ts, the shipped
// boot keeps the legacy renderer; its buttons come from the menubar's
// generator, so the empty nav and the data-menu selectors both hold.

function legacyScenario({ modelMenu } = {}) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  window.__TAURI_INTERNALS__ = {};
  installGlobals(window);
  const execCalls = [];
  window.document.execCommand = (command) => {
    execCalls.push(command);
    return true;
  };
  let agentsOpened = 0;
  const commands = setupWindowMenus({
    agents: {
      newAgent: () => {
        agentsOpened += 1;
      },
    },
    workshop: {
      toggleWorkshopPanel: () => {},
      openGatewayConfig: () => {},
      openAgentSession: () => {},
    },
    modelMenu,
  });
  const button = (id) => window.document.querySelector(`.ws-window-titlebar__menu[data-menu="${id}"]`);
  const popoverOf = (id) => button(id).nextElementSibling;
  const itemsOf = (id) => [...popoverOf(id).querySelectorAll(".ws-window-titlebar__item")];
  const itemByLabel = (id, label) =>
    itemsOf(id).find((item) => item.querySelector(".ws-window-titlebar__item-label").textContent === label);
  const isOpen = (id) => !popoverOf(id).hidden;
  return { window, commands, button, popoverOf, itemsOf, itemByLabel, isOpen, execCalls, agentsOpened: () => agentsOpened };
}

{
  const { window, button, popoverOf, itemByLabel, isOpen, agentsOpened } = legacyScenario();
  const labels = [...window.document.querySelectorAll(".ws-window-titlebar__menu")].map((b) => b.textContent);
  check(
    "the legacy setup generates the five buttons into the empty nav",
    labels.join(",") === "File,Edit,Model,Window,Help",
  );
  check(
    "the legacy renderer attaches its popover to the generated button",
    popoverOf("file")?.classList.contains("ws-window-titlebar__popover") === true,
  );
  button("file").click();
  check("the File menu opens", isOpen("file"));
  itemByLabel("file", "New Agent").click();
  check("New Agent dispatches through the agent surface", agentsOpened() === 1);
  check("running a command closes the menu", !isOpen("file"));
}

{
  const { window, button, itemsOf, itemByLabel, isOpen, execCalls } = legacyScenario();
  button("edit").click();
  check(
    "edit commands are announced disabled with no target",
    itemsOf("edit").every((item) => item.getAttribute("aria-disabled") === "true"),
  );
  itemByLabel("edit", "Undo").click();
  check("a disabled command cannot run", execCalls.length === 0 && isOpen("edit"));
  button("edit").click();
  check("clicking the open menu's button closes it", !isOpen("edit"));

  const textarea = window.document.createElement("textarea");
  window.document.body.appendChild(textarea);
  textarea.focus();
  button("edit").click();
  itemByLabel("edit", "Paste").click();
  check("Paste dispatches execCommand with a focused editable", execCalls.join(",") === "paste");
  check("the command restores focus to the preserved target", window.document.activeElement === textarea);
}

{
  const modelMenu = new ModelService(() => true);
  modelMenu.setModels([{ id: "alpha" }, { id: "beta" }]);
  modelMenu.applySelected("alpha");
  const { button, itemsOf, isOpen } = legacyScenario({ modelMenu });
  button("model").click();
  const rows = itemsOf("model");
  check(
    "the Model menu lists the catalog with the selection checked",
    isOpen("model") &&
      rows.length === 2 &&
      rows[0].getAttribute("aria-checked") === "true" &&
      rows[1].getAttribute("aria-checked") === "false",
  );
}

{
  const { window, button, itemByLabel, isOpen } = legacyScenario();
  button("help").focus();
  button("help").click();
  itemByLabel("help", "About PromptForge").click();
  check("running About closes the menu", !isOpen("help"));
  const dialog = window.document.querySelector(".ws-about-dialog");
  check("the About dialog opens as a modal dialog", dialog !== null && dialog.getAttribute("role") === "dialog" && dialog.getAttribute("aria-modal") === "true");
  if (dialog) {
    check(
      "the version line renders the build-time define",
      dialog.querySelector(".ws-about-dialog__line").textContent === "Version 0.0.0-test",
    );
    const close = dialog.querySelector(".ws-about-dialog__close");
    check("focus moves into the dialog", window.document.activeElement === close);
    window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    check("Tab stays trapped inside the dialog", window.document.activeElement === close);
    window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    check("Escape dismisses the dialog", !window.document.querySelector(".ws-about-dialog"));
    check("dismissal returns focus to the invoker", window.document.activeElement === button("help"));
  }
}

{
  const { window, commands } = legacyScenario();
  commands.dispose();
  check(
    "disposing the legacy setup removes popovers and generated buttons",
    window.document.querySelectorAll(".ws-window-titlebar__popover").length === 0 &&
      window.document.querySelectorAll(".ws-window-titlebar__menu").length === 0,
  );
}

if (failures.length > 0) {
  console.error(`window-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("window-menu: all assertions passed");
