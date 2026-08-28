// Unit test for the application menus (src/ui/window-menu.ts) and the About
// dialog (src/ui/about-dialog.ts). Bundles the TS modules with esbuild,
// imports them via data URLs, and drives them against jsdom built from the
// real index.html with the desktop flag set. Covers: menu opening,
// one-menu-at-a-time, keyboard navigation and dismissal, New Agent
// dispatch through the agent surface (the only new-conversation
// command), the Window menu's Workshop Panel toggle and its sharing of
// the visible controls' command path, the Model menu's dynamic catalog
// rows, selection marking, and empty state, the switching-state
// rendering (all rows disabled, pending mark on the switch target) and
// the live rebuild while the popover is open, Edit target preservation
// and disabled commands, the About dialog's focus trap and close, and
// the browser-mode popover wiring with inert native window commands.
// Run: node test/window-menu.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

async function bundle(entry) {
  const result = await esbuild.build({
    entryPoints: [path.join(uiDir, "..", "src", entry)],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    logLevel: "silent",
    // The modules under test import their colocated CSS; strip it - the
    // test drives only the JS, and jsdom applies no stylesheets anyway.
    loader: { ".css": "empty" },
  });
  const code = result.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const { setupWindowMenus } = await bundle(path.join("ui", "window-menu.ts"));
const { setupWindowChrome } = await bundle(path.join("ui", "window-chrome.ts"));
const { ModelService } = await bundle(path.join("services", "model-service.ts"));

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Each scenario gets a fresh jsdom: the modules read the globals and
// attach listeners to the DOM they find at call time. Pass desktop: false
// to exercise the plain-browser path (no flag, no ipc bridge).
function scenario({ desktop = true, modelMenu, profileMenu } = {}) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  const posted = [];
  if (desktop) {
    window.__PROMPTFORGE_DESKTOP__ = true;
    window.ipc = { postMessage: (message) => posted.push(JSON.parse(message)) };
  }
  const execCalls = [];
  window.document.execCommand = (command) => {
    execCalls.push(command);
    return true;
  };
  let agentsOpened = 0;
  const agents = {
    newAgent: () => {
      agentsOpened += 1;
    },
  };
  let workshopToggles = 0;
  const workshop = {
    toggleWorkshopPanel: () => {
      workshopToggles += 1;
    },
  };
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.Element = window.Element;
  globalThis.HTMLElement = window.HTMLElement;
  globalThis.HTMLInputElement = window.HTMLInputElement;
  globalThis.HTMLTextAreaElement = window.HTMLTextAreaElement;
  globalThis.Node = window.Node;
  const commands = setupWindowMenus({ agents, workshop, modelMenu, profileMenu });
  const menus = {};
  for (const button of window.document.querySelectorAll(".window-titlebar__menu")) {
    menus[button.dataset.menu] = button;
  }
  const popoverOf = (id) => menus[id].nextElementSibling;
  const itemsOf = (id) => [...popoverOf(id).querySelectorAll(".window-titlebar__item")];
  const itemByLabel = (id, label) =>
    itemsOf(id).find((item) =>
      item.querySelector(".window-titlebar__item-label").textContent === label,
    );
  const isOpen = (id) => !popoverOf(id).hidden;
  const keydown = (key) =>
    window.document.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true }));
  const stats = () => ({ agentsOpened, workshopToggles, execCalls: [...execCalls] });
  return { window, commands, menus, posted, execCalls, popoverOf, itemsOf, itemByLabel, isOpen, keydown, stats };
}

// --- Opening and one-menu-at-a-time -----------------------------------------

{
  const { menus, isOpen } = scenario();
  menus.file.click();
  check("clicking File opens its popover", isOpen("file"));
  check(
    "opening marks the button expanded",
    menus.file.getAttribute("aria-expanded") === "true",
  );
  menus.edit.click();
  check("opening Edit closes File", !isOpen("file") && isOpen("edit"));
  check(
    "the replaced button collapses",
    menus.file.getAttribute("aria-expanded") === "false",
  );
  menus.edit.click();
  check("clicking the open menu's button closes it", !isOpen("edit"));
}

// --- Keyboard navigation and dismissal ---------------------------------------

{
  const { window, menus, itemsOf, isOpen, keydown } = scenario();
  menus.edit.click();
  keydown("ArrowDown");
  check("Down focuses the first command", window.document.activeElement === itemsOf("edit")[0]);
  keydown("ArrowDown");
  check("Down again moves to the next command", window.document.activeElement === itemsOf("edit")[1]);
  keydown("ArrowUp");
  check("Up moves back", window.document.activeElement === itemsOf("edit")[0]);
  keydown("ArrowUp");
  check(
    "Up from the first command wraps to the last",
    window.document.activeElement === itemsOf("edit")[itemsOf("edit").length - 1],
  );
  keydown("ArrowRight");
  check("Right opens the next menu", !isOpen("edit") && isOpen("model"));
  check(
    "Right focuses the new menu's first row",
    window.document.activeElement === itemsOf("model")[0],
  );
  keydown("ArrowLeft");
  check("Left opens the previous menu", isOpen("edit") && !isOpen("model"));
  keydown("Escape");
  check("Escape closes the menu", !isOpen("edit"));
  check(
    "Escape returns focus to the menu button",
    window.document.activeElement === menus.edit,
  );
  menus.file.click();
  window.document.body.dispatchEvent(new window.MouseEvent("pointerdown", { bubbles: true }));
  check("outside pointerdown dismisses the menu", !isOpen("file"));
}

// --- File menu commands -------------------------------------------------------

{
  const { menus, itemByLabel, isOpen, posted, stats } = scenario();
  menus.file.click();
  check("New Chat is gone from the File menu", itemByLabel("file", "New Chat") === undefined);
  itemByLabel("file", "New Agent").click();
  check("New Agent dispatches the agent surface's newAgent", stats().agentsOpened === 1);
  check("running a command closes the menu", !isOpen("file"));
  menus.file.click();
  itemByLabel("file", "Close Window").click();
  check(
    "Close Window posts the typed close envelope",
    posted.map((message) => message.command).join(",") === "close",
  );
}

// --- Window menu: Workshop Panel toggle and the shared command path ----------

{
  const { window, menus, itemByLabel, isOpen, posted, stats } = scenario();
  setupWindowChrome();
  menus.window.click();
  const workshopItem = itemByLabel("window", "Workshop Panel");
  check("the Window menu lists Workshop Panel", workshopItem !== undefined);
  check(
    "Workshop Panel shows the Ctrl+B shortcut hint",
    workshopItem?.querySelector(".window-titlebar__shortcut")?.textContent === "Ctrl+B",
  );
  workshopItem.click();
  check("Workshop Panel dispatches the workshop toggle", stats().workshopToggles === 1);
  check("running Workshop Panel closes the menu", !isOpen("window"));
  const visible = (command) => window.document.querySelector(`[data-command="${command}"]`);
  visible("minimize").click();
  menus.window.click();
  itemByLabel("window", "Minimize").click();
  visible("toggle-maximize").click();
  menus.window.click();
  itemByLabel("window", "Maximize/Restore").click();
  const sent = posted.map((message) => JSON.stringify(message));
  check(
    "menu and visible controls post identical envelopes",
    sent.join("|") ===
      [
        JSON.stringify({ command: "minimize" }),
        JSON.stringify({ command: "minimize" }),
        JSON.stringify({ command: "toggle-maximize" }),
        JSON.stringify({ command: "toggle-maximize" }),
      ].join("|"),
  );
}

// --- Edit menu: disabled without a target, preserved target with one ---------

{
  const { window, menus, itemsOf, itemByLabel, isOpen, execCalls, keydown } = scenario();
  menus.edit.click();
  check(
    "edit commands are announced disabled with no target",
    itemsOf("edit").every((item) => item.getAttribute("aria-disabled") === "true"),
  );
  itemByLabel("edit", "Undo").click();
  check("a disabled command cannot run", execCalls.length === 0);
  check("clicking a disabled command keeps the menu open", isOpen("edit"));
  keydown("Escape");

  const textarea = window.document.createElement("textarea");
  window.document.body.appendChild(textarea);
  textarea.focus();
  menus.edit.click();
  check(
    "edit commands enable with a focused editable",
    itemsOf("edit").every((item) => item.getAttribute("aria-disabled") === "false"),
  );
  itemByLabel("edit", "Paste").click();
  check("Paste dispatches execCommand", execCalls.join(",") === "paste");
  check(
    "the command restores focus to the preserved target",
    window.document.activeElement === textarea,
  );
  menus.edit.click();
  keydown("ArrowDown");
  keydown("ArrowDown");
  keydown("Enter");
  check("Enter activates the focused command", execCalls.join(",") === "paste,redo");
  check(
    "Enter restores focus to the preserved target",
    window.document.activeElement === textarea,
  );
}

// --- Model menu: dynamic catalog rows ----------------------------------------

{
  const catalog = [
    { id: "alpha", description: "the alpha model" },
    { id: "beta" },
  ];
  const selections = [];
  const modelMenu = new ModelService((id) => (selections.push(id), true));
  modelMenu.setModels(catalog);
  // The selection is server-owned: it arrives as a snapshot, never from
  // the catalog itself.
  modelMenu.applySelected("alpha");
  const { menus, itemsOf, isOpen } = scenario({ modelMenu });
  menus.model.click();
  check("the Model menu opens", isOpen("model"));
  const rows = itemsOf("model");
  const rowLabel = (row) => row.querySelector(".window-titlebar__item-label").textContent;
  check(
    "the Model menu lists the catalog entries",
    rows.map(rowLabel).join(",") === "alpha,beta",
  );
  check(
    "the selected model is announced checked",
    rows[0].getAttribute("aria-checked") === "true" &&
      rows[1].getAttribute("aria-checked") === "false",
  );
  check(
    "the selected model shows the checkmark",
    rows[0].querySelector(".window-titlebar__item-check").textContent === "✓" &&
      rows[1].querySelector(".window-titlebar__item-check").textContent === "",
  );
  check(
    "the model description becomes the row tooltip",
    rows[0].title === "the alpha model",
  );
  rows[1].click();
  check("clicking a model row sends the select command", selections.join(",") === "beta");
  check("selecting a model closes the menu", !isOpen("model"));
  // The server answers the command with a snapshot; only then does the
  // selection move.
  modelMenu.applySelected("beta");
  menus.model.click();
  check(
    "the rebuilt menu marks the new selection",
    itemsOf("model")[1].getAttribute("aria-checked") === "true",
  );
}

{
  const { menus, itemsOf, isOpen } = scenario({ modelMenu: new ModelService(() => true) });
  menus.model.click();
  const rows = itemsOf("model");
  check(
    "an empty catalog shows one disabled row",
    rows.length === 1 &&
      rows[0].querySelector(".window-titlebar__item-label").textContent === "No models available" &&
      rows[0].getAttribute("aria-disabled") === "true",
  );
  rows[0].click();
  check("clicking the empty-state row keeps the menu open", isOpen("model"));
}

// --- Model menu: gateway profiles section -------------------------------------

{
  const modelMenu = new ModelService(() => true);
  modelMenu.setModels([{ id: "alpha" }]);
  const switches = [];
  const profileMenu = {
    profiles: ["main", "qwen38"],
    active: "main",
    switchTo: (name) => switches.push(name),
  };
  const { menus, popoverOf, itemsOf, isOpen } = scenario({ modelMenu, profileMenu });
  menus.model.click();
  const rows = itemsOf("model");
  const rowLabel = (row) => row.querySelector(".window-titlebar__item-label").textContent;
  check(
    "the Model menu appends the Profiles section after the catalog",
    rows.map(rowLabel).join(",") === "alpha,Profiles,main,qwen38",
  );
  check(
    "the sections are divided by a separator",
    popoverOf("model").querySelector(".window-titlebar__separator") !== null,
  );
  check(
    "the Profiles header is an inert label",
    rows[1].getAttribute("aria-disabled") === "true",
  );
  check(
    "the active profile is announced checked",
    rows[2].getAttribute("aria-checked") === "true" &&
      rows[3].getAttribute("aria-checked") === "false",
  );
  check(
    "the active profile shows the checkmark",
    rows[2].querySelector(".window-titlebar__item-check").textContent === "✓" &&
      rows[3].querySelector(".window-titlebar__item-check").textContent === "",
  );
  rows[3].click();
  check("clicking a profile row dispatches switchTo", switches.join(",") === "qwen38");
  check("switching closes the menu", !isOpen("model"));
}

{
  const modelMenu = new ModelService(() => true);
  modelMenu.setModels([{ id: "alpha" }]);
  const profileMenu = { profiles: ["main"], active: "main", switchTo: () => {} };
  const { menus, itemsOf } = scenario({ modelMenu, profileMenu });
  menus.model.click();
  check(
    "a single-profile gateway shows no Profiles section",
    itemsOf("model").length === 1,
  );
}

{
  // A profile whose catalog is empty still offers the way out: the
  // Profiles section renders below the empty state, so a switch can
  // restore a usable catalog.
  const switches = [];
  const profileMenu = {
    profiles: ["main", "qwen38"],
    active: "qwen38",
    switchTo: (name) => switches.push(name),
  };
  const { menus, itemsOf } = scenario({ modelMenu: new ModelService(() => true), profileMenu });
  menus.model.click();
  const rows = itemsOf("model");
  const rowLabel = (row) => row.querySelector(".window-titlebar__item-label").textContent;
  check(
    "an empty catalog still lists the profiles",
    rows.map(rowLabel).join(",") === "No models available,Profiles,main,qwen38",
  );
  rows[2].click();
  check("a profile can be switched out of an empty catalog", switches.join(",") === "main");
}

// --- Model menu: a switch in flight -------------------------------------------

{
  const selections = [];
  const modelMenu = new ModelService((id) => (selections.push(id), true));
  modelMenu.setModels([{ id: "alpha" }]);
  modelMenu.applySelected("alpha");
  const switches = [];
  const profileMenu = {
    profiles: ["main", "qwen38"],
    active: "main",
    switching: "qwen38",
    switchTo: (name) => switches.push(name),
  };
  const { menus, itemsOf, isOpen } = scenario({ modelMenu, profileMenu });
  menus.model.click();
  const rows = itemsOf("model");
  const markOf = (row) => row.querySelector(".window-titlebar__item-check");
  check(
    "a switch in flight disables every model and profile row",
    rows.every((row) => row.getAttribute("aria-disabled") === "true"),
  );
  check(
    "the switch target shows the pending mark instead of a check",
    markOf(rows[3]).textContent === "…" &&
      markOf(rows[3]).classList.contains("window-titlebar__item-check--pending"),
  );
  check(
    "the still-active profile keeps its check while the switch runs",
    markOf(rows[2]).textContent === "✓" &&
      rows[2].getAttribute("aria-checked") === "true" &&
      rows[3].getAttribute("aria-checked") === "false",
  );
  check(
    "the pending row stays a radio item for assistive tech",
    rows[3].getAttribute("role") === "menuitemradio",
  );
  rows[3].click();
  check("a disabled profile row cannot dispatch another switch", switches.length === 0);
  rows[0].click();
  check("a disabled model row cannot send a selection", selections.length === 0);
  check("clicking disabled rows keeps the menu open", isOpen("model"));
}

// --- Model menu: live rebuild while the popover is open -----------------------

{
  const modelMenu = new ModelService(() => true);
  modelMenu.setModels([{ id: "alpha" }]);
  const listeners = new Set();
  const profileMenu = {
    profiles: ["main", "qwen38"],
    active: "main",
    switching: "",
    onDidChange: (listener) => {
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
    switchTo: () => {},
  };
  const { menus, itemsOf, isOpen } = scenario({ modelMenu, profileMenu });
  menus.model.click();
  check("opening the Model menu subscribes to workbench changes", listeners.size === 1);
  // The server starts the switch: a switching=<target> snapshot arrives
  // while the popover is open.
  profileMenu.switching = "qwen38";
  for (const listener of listeners) listener();
  let rows = itemsOf("model");
  const markOf = (row) => row.querySelector(".window-titlebar__item-check");
  check(
    "a snapshot while open disables the rows without reopening",
    isOpen("model") && rows.every((row) => row.getAttribute("aria-disabled") === "true"),
  );
  check("the pending mark appears without reopening", markOf(rows[3]).textContent === "…");
  // The switch completes: the final snapshot restores truth, including
  // the server-owned model selection.
  profileMenu.switching = "";
  profileMenu.active = "qwen38";
  modelMenu.applySelected("alpha");
  for (const listener of listeners) listener();
  rows = itemsOf("model");
  check(
    "the completing snapshot moves the check to the new profile",
    rows[3].getAttribute("aria-checked") === "true" &&
      markOf(rows[3]).textContent === "✓" &&
      rows[2].getAttribute("aria-checked") === "false",
  );
  check(
    "rows re-enable when the switch completes",
    rows[0].getAttribute("aria-disabled") === "false" &&
      rows[2].getAttribute("aria-disabled") === "false",
  );
  check(
    "the checked model row tracks the snapshot selection",
    rows[0].getAttribute("aria-checked") === "true" && markOf(rows[0]).textContent === "✓",
  );
  menus.model.click();
  check(
    "closing the menu disposes the workbench subscription",
    !isOpen("model") && listeners.size === 0,
  );
}

// --- Help menu: About dialog --------------------------------------------------

{
  const { window, menus, itemByLabel, isOpen } = scenario();
  menus.help.focus();
  menus.help.click();
  itemByLabel("help", "About PromptForge").click();
  check("running About closes the menu", !isOpen("help"));
  const dialog = window.document.querySelector(".about-dialog");
  check("the About dialog opens", dialog !== null);
  if (dialog) {
    check("the dialog is a modal dialog", dialog.getAttribute("role") === "dialog" &&
      dialog.getAttribute("aria-modal") === "true");
    const text = dialog.textContent;
    check(
      "the dialog names product, version, and license",
      text.includes("PromptForge") && text.includes("0.1.0") && text.includes("BSL-1.0"),
    );
    const close = dialog.querySelector(".about-dialog__close");
    check("focus moves into the dialog", window.document.activeElement === close);
    window.document.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Tab", bubbles: true }),
    );
    check("Tab stays trapped inside the dialog", window.document.activeElement === close);
    window.document.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }),
    );
    check("Shift+Tab stays trapped inside the dialog", window.document.activeElement === close);
    window.document.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    check("Escape dismisses the dialog", !window.document.querySelector(".about-dialog"));
    check(
      "dismissal returns focus to the invoker",
      window.document.activeElement === menus.help,
    );
  }
}

// --- The command set is shared with future surfaces ---------------------------

{
  const { commands, stats } = scenario();
  check("setup returns the shared command set", typeof commands.newAgent === "function" &&
    typeof commands.toggleWorkshopPanel === "function" &&
    typeof commands.minimizeWindow === "function" &&
    typeof commands.showAbout === "function");
  check("newChat is gone from the shared command set", !("newChat" in commands));
  commands.newAgent();
  check("the shared set dispatches New Agent", stats().agentsOpened === 1);
  commands.toggleWorkshopPanel();
  check("the shared set dispatches the workshop toggle", stats().workshopToggles === 1);
}

// --- Browser mode: popovers wired, native window commands inert --------------

{
  const { window, commands, menus, itemByLabel, isOpen, posted, stats } = scenario({ desktop: false });
  check(
    "browser mode builds every popover",
    window.document.querySelectorAll(".window-titlebar__popover").length === 5,
  );
  menus.file.click();
  check("clicking File opens its popover in browser mode", isOpen("file"));
  itemByLabel("file", "New Agent").click();
  check("browser mode commands dispatch", stats().agentsOpened === 1);
  check("running a command closes the menu in browser mode", !isOpen("file"));
  menus.window.click();
  itemByLabel("window", "Minimize").click();
  menus.file.click();
  itemByLabel("file", "Close Window").click();
  check(
    "native window commands no-op without the IPC bridge",
    posted.length === 0 && !("ipc" in window),
  );
  commands.newAgent();
  check("browser mode still returns a working command set", stats().agentsOpened === 2);
}

if (failures.length > 0) {
  console.error(`window-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("window-menu: all assertions passed");
