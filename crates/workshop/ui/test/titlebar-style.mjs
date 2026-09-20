// Title-bar built-artifact contract: loads the bundled stylesheet and
// shipped markup into jsdom, mounts the menubar (src/parts/menu/menubar.ts)
// over the shipped empty nav with the eight top-level menus registered,
// then checks the region structure, the generated buttons, visibility,
// sizing, glyph, and keyboard-focus behavior that jsdom can execute
// without a layout engine.
// Run after `npm run build`: `node --test test/titlebar-style.mjs`.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "..", "dist");
// The bundled stylesheet's name is content-hashed; the build's manifest
// maps the logical name to it.
const manifest = JSON.parse(await readFile(path.join(distDir, "manifest.json"), "utf8"));
const [html, css] = await Promise.all([
  readFile(path.join(distDir, "index.html"), "utf8"),
  readFile(path.join(distDir, manifest["app.css"]), "utf8"),
]);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

check("the production stylesheet bundle is nonempty", css.length > 0);

const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
const { window } = dom;
const style = window.document.createElement("style");
style.textContent = css;
window.document.head.appendChild(style);

const barEl = window.document.querySelector(".ws-window-titlebar");
check("title bar present in the shipped markup", barEl !== null);

// --- Region structure: __left (icon + menus), __center (drag), __right (controls) ---

if (barEl) {
  const left = barEl.querySelector(":scope > .ws-window-titlebar__left");
  const center = barEl.querySelector(":scope > .ws-window-titlebar__center");
  const right = barEl.querySelector(":scope > .ws-window-titlebar__right");
  check("the bar splits into left, center, and right regions", left !== null && center !== null && right !== null);
  if (left) {
    check("the left region carries the program icon", left.querySelector(".ws-window-titlebar__icon") !== null);
    const nav = left.querySelector(".ws-window-titlebar__menus");
    check(
      "the left region carries the menubar nav as a menubar landmark",
      nav !== null && nav.getAttribute("role") === "menubar" && nav.getAttribute("aria-label") === "Application menus",
    );
    check("the menubar nav ships empty; the buttons are generated", nav !== null && nav.children.length === 0);
  }
  if (center) {
    check("the center region is the drag surface", center.classList.contains("ws-window-titlebar__drag"));
  }
  if (right) {
    check("the right region carries the window controls", right.querySelector(".ws-window-titlebar__controls") !== null);
  }
}

// --- The menubar generates the eight top-level buttons ------------------------------

// The shipped nav is empty; mount the real Menubar over it with the
// eight top-level menus registered, as the menubar contribution does at
// boot. The bundle reads the globals, so point them at this jsdom first.
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { Menubar } from "./src/parts/menu/menubar.ts";
      export { CommandRegistry } from "./src/services/command-registry.ts";
      export { MenuRegistry, MenuId } from "./src/services/menu-registry.ts";
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
const { Menubar, CommandRegistry, MenuRegistry, MenuId, ContextKeyService, createKeybindingsRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

if (barEl) {
  const menus = new MenuRegistry();
  const topLevel = [
    [MenuId.MenubarFileMenu, "File"],
    [MenuId.MenubarEditMenu, "Edit"],
    [MenuId.MenubarSelectionMenu, "Selection"],
    [MenuId.MenubarViewMenu, "View"],
    [MenuId.MenubarGoMenu, "Go"],
    [MenuId.MenubarRunMenu, "Run"],
    [MenuId.MenubarTerminalMenu, "Terminal"],
    [MenuId.MenubarHelpMenu, "Help"],
  ];
  topLevel.forEach(([id, title], index) => {
    menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: id, title, order: index + 1 });
  });
  const nav = barEl.querySelector(".ws-window-titlebar__menus");
  new Menubar(nav, {
    menus,
    commands: new CommandRegistry(),
    contextKeys: new ContextKeyService(),
    keybindings: createKeybindingsRegistry("linux"),
  });
  const buttons = [...nav.querySelectorAll(".ws-window-titlebar__menu")];
  check(
    "the menubar generates the eight top-level buttons in order",
    buttons.map((button) => button.textContent).join(",") === "File,Edit,Selection,View,Go,Run,Terminal,Help",
  );
  check(
    "the buttons carry the last-segment data-menu selectors",
    buttons.map((button) => button.dataset.menu).join(",") === "file,edit,selection,view,go,run,terminal,help",
  );
}

// --- Visibility, sizing, controls, glyphs, focus -------------------------------------

if (barEl) {
  const order = [...barEl.querySelectorAll(".ws-window-titlebar__control")].map((button) =>
    button.getAttribute("aria-label"),
  );
  check(
    "window controls are ordered Minimize, Maximize, Close",
    order.join(",") === "Minimize,Maximize,Close",
  );
  check(
    "the [hidden] bar computes to display:none in browser mode",
    window.getComputedStyle(barEl).display === "none",
  );
  barEl.hidden = false;
  const revealed = window.getComputedStyle(barEl);
  check("the revealed bar computes to the flex row", revealed.display === "flex");
  // jsdom reports the declared var() expression for height but resolves
  // custom properties themselves; together they prove the fixed height
  // flows through the variable.
  check(
    "the revealed bar's height is declared via --titlebar-height",
    revealed.height.startsWith("var(--titlebar-height"),
  );
  check(
    "--titlebar-height resolves to a fixed pixel value",
    /^\d+px$/.test(revealed.getPropertyValue("--titlebar-height").trim()),
  );
  for (const button of barEl.querySelectorAll("button")) {
    button.focus();
    check(
      `${button.textContent.trim() || button.getAttribute("aria-label")} accepts keyboard focus`,
      window.document.activeElement === button && button.matches(":focus"),
    );
    check(
      `${button.textContent.trim() || button.getAttribute("aria-label")} focus has no browser outline`,
      window.getComputedStyle(button).outlineStyle === "none",
    );
  }
  const restoreGlyph = barEl.querySelector(".ws-window-titlebar__glyph--restore");
  const maximizeGlyph = barEl.querySelector(".ws-window-titlebar__glyph--maximize");
  check(
    "the restore glyph ships with the hidden attribute",
    restoreGlyph !== null && restoreGlyph.hasAttribute("hidden"),
  );
  if (restoreGlyph && maximizeGlyph) {
    check(
      "the [hidden] restore glyph computes to display:none",
      window.getComputedStyle(restoreGlyph).display === "none",
    );
    check(
      "the visible maximize glyph does not compute to display:none",
      window.getComputedStyle(maximizeGlyph).display !== "none",
    );
  }
}

if (failures.length > 0) {
  console.error(`titlebar-style: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("titlebar-style: all assertions passed");
