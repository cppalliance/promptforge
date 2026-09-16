// Unit test for the command center (src/ui/chrome/command-center.ts): the
// title-bar toolbar over MenuId.CommandCenter, mounted inside the center
// drag region with the no-drag marker. The built-in pill carries the
// search icon and window title and dispatches the menu's first command
// row; the ? chevron runs workbench.action.quickOpenHelp; further
// command rows render as toolbar buttons. Also covers the pill's
// aria-label and the WindowTitle helper showing the first granted root's
// folder name (falling back to "PromptForge" when no root is granted or
// the listing fails), re-rendering on the workspace-changed event, and
// tracking document.title.
// Bundles the module with esbuild and drives it against jsdom.
// Run: node --test test/command-center.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM(
  `<header class="ws-window-titlebar">
     <div class="ws-window-titlebar__left"></div>
     <div class="ws-window-titlebar__center ws-window-titlebar__drag"></div>
     <div class="ws-window-titlebar__right"></div>
   </header>`,
  { url: "http://127.0.0.1:7910/" },
);
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
      export { CommandCenter, WindowTitle } from "./src/ui/chrome/command-center.ts";
      export { CommandRegistry } from "./src/services/command-registry.ts";
      export { MenuRegistry, MenuId } from "./src/services/menu-registry.ts";
      export { WORKSPACE_CHANGED_EVENT } from "./src/ui/workspace/workspace-drops.ts";
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
const { CommandCenter, WindowTitle, CommandRegistry, MenuRegistry, MenuId, WORKSPACE_CHANGED_EVENT } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

// --- Fixture: a command center in the title bar's center drag region -------

const center = window.document.querySelector(".ws-window-titlebar__center");
let roots = [{ name: "promptforge" }];
const commands = new CommandRegistry();
const ran = [];
commands.register("workbench.action.quickOpenWithModes", { run: () => ran.push("modes") });
commands.register("workbench.action.quickOpenHelp", { run: () => ran.push("help") });
commands.register("example.extra", { title: "Extra Row", run: () => ran.push("extra") });
const menus = new MenuRegistry();
menus.appendMenuItem(MenuId.CommandCenter, { command: "workbench.action.quickOpenWithModes" });
menus.appendMenuItem(MenuId.CommandCenter, { command: "example.extra", group: "1_extra" });

const commandCenter = new CommandCenter(center, { commands, menus, listRoots: async () => roots });
await flush();

// --- Mounting and no-drag placement ----------------------------------------

const container = center.querySelector(".ws-command-center");
check("the command center mounts inside the center drag region", container !== null);
check(
  "the command center carries the no-drag marker",
  container?.classList.contains("ws-window-titlebar__no-drag") === true,
);

const pill = container.querySelector(".ws-command-center__pill");
check("the pill is a button", pill?.tagName === "BUTTON");
check("the pill is a type=button control", pill?.type === "button");
check(
  "the pill carries its aria-label",
  pill?.getAttribute("aria-label") === "Search files, commands, and more",
);
check(
  "the pill opens with a decorative search icon",
  pill?.querySelector(".ws-command-center__search")?.getAttribute("aria-hidden") === "true",
);

// --- Window title: first granted root, then document.title -----------------

check("the pill shows the first granted root's folder name", pill.textContent.includes("promptforge"));
check("document.title tracks the folder name", window.document.title === "promptforge");

// --- Click routing ----------------------------------------------------------

pill.click();
await flush();
check("clicking the pill opens quick open with modes", ran.length === 1 && ran[0] === "modes");

const chevron = container.querySelector(".ws-command-center__chevron");
check("the chevron is a type=button control", chevron?.tagName === "BUTTON" && chevron?.type === "button");
chevron.click();
await flush();
check("clicking the chevron opens the ? help", ran.length === 2 && ran[1] === "help");

// --- Extra menu rows render as toolbar buttons ------------------------------

const extraRow = container.querySelector(".ws-command-center__item");
check(
  "a second menu row renders as a toolbar button",
  extraRow?.tagName === "BUTTON" && extraRow?.type === "button",
);
check("the extra row takes the command's title", extraRow?.textContent === "Extra Row");
extraRow.click();
await flush();
check("clicking the extra row dispatches its command", ran.length === 3 && ran[2] === "extra");

// --- Grant changes re-render the title --------------------------------------

roots = [{ name: "other-folder" }];
window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
await flush();
check("a grant change re-renders the title", pill.textContent.includes("other-folder"));
check("document.title follows the grant change", window.document.title === "other-folder");

roots = [];
window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
await flush();
check("no granted roots falls back to PromptForge", pill.textContent.includes("PromptForge"));
check("document.title falls back to PromptForge", window.document.title === "PromptForge");

// --- A failing roots listing keeps the fallback -----------------------------

const secondCenter = window.document.createElement("div");
window.document.body.appendChild(secondCenter);
const failing = new CommandCenter(secondCenter, {
  commands,
  listRoots: async () => {
    throw new Error("server down");
  },
});
await flush();
const failingPill = secondCenter.querySelector(".ws-command-center__pill");
check("a failed listing falls back to PromptForge", failingPill.textContent.includes("PromptForge"));
failing.dispose();
check("dispose removes the command center", secondCenter.querySelector(".ws-command-center") === null);

// --- No registered rows: the pill's click dispatches nothing -----------------

const emptyHost = window.document.createElement("div");
window.document.body.appendChild(emptyHost);
const noRows = new CommandCenter(emptyHost, {
  commands,
  menus: new MenuRegistry(),
  listRoots: async () => roots,
});
await flush();
emptyHost.querySelector(".ws-command-center__pill").click();
await flush();
check("an empty menu leaves the pill inert", ran.length === 3);
noRows.dispose();

// --- WindowTitle stands alone ------------------------------------------------

let standaloneRoots = [{ name: "solo" }];
const title = new WindowTitle({ listRoots: async () => standaloneRoots });
await flush();
check("WindowTitle exposes its element", title.element instanceof window.HTMLElement);
check("WindowTitle sets document.title on its own", window.document.title === "solo");
title.dispose();

commandCenter.dispose();
check("disposing the command center removes its container", center.querySelector(".ws-command-center") === null);

if (failures.length > 0) {
  console.error(`command-center: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("command-center: all assertions passed");
