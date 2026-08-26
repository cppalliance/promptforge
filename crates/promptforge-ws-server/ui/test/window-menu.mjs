// Unit test for the application menus (src/window-menu.ts) and the About
// dialog (src/about-dialog.ts). Bundles the TS modules with esbuild,
// imports them via data URLs, and drives them against jsdom built from the
// real index.html with the desktop flag set. Covers: menu opening,
// one-menu-at-a-time, keyboard navigation and dismissal, New Chat
// dispatch, the Window menu sharing the visible controls' command path,
// the Window menu's layout-lock command and its state-tracking label,
// Edit target preservation and disabled commands, the About dialog's
// focus trap and close, and the browser-mode skip of the popover wiring.
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
  });
  const code = result.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const { setupWindowMenus } = await bundle("window-menu.ts");
const { setupWindowChrome } = await bundle("window-chrome.ts");

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Each scenario gets a fresh jsdom: the modules read the globals and
// attach listeners to the DOM they find at call time. Pass desktop: false
// to exercise the plain-browser path (no flag, no ipc bridge).
function scenario({ desktop = true, layoutLock } = {}) {
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
  let created = 0;
  const chat = {
    engine: {
      sessions: {
        create: () => {
          created += 1;
          return Promise.resolve();
        },
      },
    },
  };
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.Element = window.Element;
  globalThis.HTMLElement = window.HTMLElement;
  globalThis.HTMLInputElement = window.HTMLInputElement;
  globalThis.HTMLTextAreaElement = window.HTMLTextAreaElement;
  globalThis.Node = window.Node;
  const commands = setupWindowMenus({ chat, layoutLock });
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
  const stats = () => ({ created: created, execCalls: [...execCalls] });
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
  check("Right opens the next menu", !isOpen("edit") && isOpen("window"));
  check(
    "Right focuses the new menu's first command",
    window.document.activeElement === itemsOf("window")[0],
  );
  keydown("ArrowLeft");
  check("Left opens the previous menu", isOpen("edit") && !isOpen("window"));
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
  itemByLabel("file", "New Chat").click();
  check("New Chat dispatches sessions.create", stats().created === 1);
  check("running a command closes the menu", !isOpen("file"));
  menus.file.click();
  itemByLabel("file", "Close Window").click();
  check(
    "Close Window posts the typed close envelope",
    posted.map((message) => message.command).join(",") === "close",
  );
}

// --- Window menu shares the visible controls' command path -------------------

{
  const { window, menus, itemByLabel, posted } = scenario();
  setupWindowChrome();
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

// --- Window menu: the layout-lock command ------------------------------------

{
  const { menus, itemByLabel } = scenario();
  menus.window.click();
  check(
    "the Window menu omits the lock command without a lock surface",
    itemByLabel("window", "Unlock Layout") === undefined &&
      itemByLabel("window", "Lock Layout") === undefined,
  );
}

{
  let locked = true;
  let toggles = 0;
  const { menus, itemByLabel, isOpen } = scenario({
    layoutLock: {
      isLocked: () => locked,
      toggle: () => {
        toggles += 1;
        locked = !locked;
      },
    },
  });
  menus.window.click();
  check(
    "the lock command offers Unlock while locked",
    itemByLabel("window", "Unlock Layout") !== undefined,
  );
  itemByLabel("window", "Unlock Layout").click();
  check("the lock command dispatches the toggle", toggles === 1);
  check("running the lock command closes the menu", !isOpen("window"));
  menus.window.click();
  check(
    "the lock label tracks the unlocked state on reopen",
    itemByLabel("window", "Lock Layout") !== undefined,
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
  check("setup returns the shared command set", typeof commands.newChat === "function" &&
    typeof commands.minimizeWindow === "function" &&
    typeof commands.showAbout === "function");
  commands.newChat();
  check("the shared set dispatches New Chat", stats().created === 1);
}

// --- Browser mode: no popovers, commands still dispatch -----------------------

{
  const { window, commands, stats } = scenario({ desktop: false });
  check(
    "browser mode builds no popovers",
    window.document.querySelectorAll(".window-titlebar__popover").length === 0,
  );
  commands.newChat();
  check("browser mode still returns a working command set", stats().created === 1);
}

if (failures.length > 0) {
  console.error(`window-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("window-menu: all assertions passed");
