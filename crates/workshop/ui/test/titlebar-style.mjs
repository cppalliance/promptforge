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
    check("the left region holds the program icon", left.querySelector(".ws-window-titlebar__icon") !== null);
    const nav = left.querySelector(".ws-window-titlebar__menus");
    check(
      "the left region holds the menubar nav as a menubar landmark",
      nav !== null && nav.getAttribute("role") === "menubar" && nav.getAttribute("aria-label") === "Application menus",
    );
    check("the menubar nav ships empty; the buttons are generated", nav !== null && nav.children.length === 0);
  }
  if (center) {
    check("the center region is the drag surface", center.classList.contains("ws-window-titlebar__drag"));
  }
  if (right) {
    check("the right region holds the window controls", right.querySelector(".ws-window-titlebar__controls") !== null);
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
      export { CommandRegistry } from "@workshop/platform/command-registry";
      export { MenuRegistry, MenuId } from "@workshop/platform/menu-registry";
      export { ContextKeyService } from "@workshop/platform/context-key-service";
      export { createKeybindingsRegistry } from "@workshop/platform/keybinding-registry";
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
    "the buttons have the last-segment data-menu selectors",
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

// --- Cursor's title-bar skin, read from the source stylesheets --------------------
//
// jsdom applies no stylesheets for var() resolution, so the skin's values are
// read from the sources: the palette tokens in @workshop/look, the window
// controls, icon box, toolbars, and zoom counter-scaling in window-chrome.css,
// and the folder-name button in command-center.css.

const stripComments = (source) => source.replace(/\/\*[\s\S]*?\*\//g, "");
const rulesOf = (source) =>
  [...stripComments(source).matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
    selectors: match[1].split(",").map((selector) => selector.replace(/\s+/g, " ").trim()),
    body: match[2].replace(/\s+/g, " ").trim(),
  }));
/** Every declaration the rules naming exactly `selector` carry, concatenated. */
const bodyOf = (rules, selector) =>
  rules
    .filter((rule) => rule.selectors.includes(selector))
    .map((rule) => rule.body)
    .join(" ");
const declares = (body, property, value) =>
  new RegExp(`(?:^|[\\s;])${property}:\\s*${value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*(?:;|$)`).test(body);

const lookTokens = await readFile(path.join(uiDir, "..", "..", "look", "tokens.css"), "utf8");
const tokenValue = (name) => new RegExp(`${name}:\\s*([^;]+);`).exec(stripComments(lookTokens))?.[1].trim();
check("the window-control glyphs are #F0F0F084", tokenValue("--titlebar-glyph") === "#F0F0F084");
check("a window control's hover is #FFFFFF1A", tokenValue("--titlebar-control-hover") === "#FFFFFF1A");
check("the close control's hover is #E81123E6", tokenValue("--titlebar-close-hover") === "#E81123E6");
check("an inactive window's title-bar text is #F0F0F099", tokenValue("--titlebar-foreground-inactive") === "#F0F0F099");
check("the folder-name button's hover is #FFFFFF14", tokenValue("--titlebar-folder-hover") === "#FFFFFF14");
check("the app icon is 16px", tokenValue("--titlebar-icon-size") === "16px");
check("the app icon's box is the 35px bar height", tokenValue("--titlebar-icon-box") === "var(--titlebar-height)" && tokenValue("--titlebar-height") === "35px");
check("the toolbar buttons are 22px", tokenValue("--titlebar-tool-size") === "22px");

const chromeRules = rulesOf(await readFile(path.join(uiDir, "..", "src", "parts", "chrome", "window-chrome.css"), "utf8"));
check(
  "an inactive window dims the bar to its inactive text color",
  declares(bodyOf(chromeRules, ".ws-window-titlebar--inactive"), "color", "var(--titlebar-foreground-inactive)"),
);
check(
  "the window controls divide the zoom factor out to keep their physical size",
  declares(bodyOf(chromeRules, ".ws-window-titlebar__controls"), "zoom", "calc(1 / var(--ws-zoom-factor, 1))"),
);
check(
  "minimize and maximize hover with the control wash",
  declares(bodyOf(chromeRules, ".ws-window-titlebar__control--minimize:hover"), "background", "var(--titlebar-control-hover)"),
);
check(
  "close hovers red",
  declares(bodyOf(chromeRules, ".ws-window-titlebar__control--close:hover"), "background", "var(--titlebar-close-hover)"),
);
const iconBody = bodyOf(chromeRules, ".ws-window-titlebar__icon");
check(
  "the app icon is 16px, centered in a box the bar's height, flush to the left edge",
  declares(iconBody, "width", "var(--titlebar-icon-size)") &&
    declares(iconBody, "height", "var(--titlebar-icon-size)") &&
    declares(iconBody, "box-sizing", "content-box") &&
    declares(iconBody, "padding", "calc((var(--titlebar-icon-box) - var(--titlebar-icon-size)) / 2)") &&
    declares(iconBody, "margin", "0"),
);
const toolBody = bodyOf(chromeRules, ".ws-window-titlebar__tool");
check(
  "a toolbar button is a 22px square with a 6px-radius toolbar-hover wash",
  declares(toolBody, "width", "var(--titlebar-tool-size)") &&
    declares(toolBody, "height", "var(--titlebar-tool-size)") &&
    declares(bodyOf(chromeRules, ".ws-window-titlebar__tool:hover"), "background", "var(--cursor-toolbar-hover)"),
);

const centerRules = rulesOf(await readFile(path.join(uiDir, "..", "src", "parts", "chrome", "command-center.css"), "utf8"));
const folderBody = bodyOf(centerRules, ".ws-command-center__folder");
check(
  "the folder-name button is 12px text with 1px 6px padding and a 6px radius",
  declares(folderBody, "font-size", "var(--font-size-sm)") &&
    declares(folderBody, "padding", "var(--ws-size-1) var(--ws-size-6)") &&
    declares(folderBody, "border-radius", "var(--radius)"),
);
check(
  "the folder-name button hovers with the folder wash",
  declares(bodyOf(centerRules, ".ws-command-center__folder:hover"), "background", "var(--titlebar-folder-hover)"),
);

// --- Cursor's menu skin, read from the source stylesheet --------------------------

const menuRules = rulesOf(await readFile(path.join(uiDir, "..", "src", "parts", "menu", "window-menu.css"), "utf8"));
// The menu label's color must come from the bar, or an inactive window dims
// the folder-name button but leaves the menu labels at the active color.
// jsdom resolves neither var() nor inherited color, so the contract is read
// from the sources: the label declares no color but `inherit`, the bar sets
// the active color, and the inactive rule overrides it on the bar.
check(
  "a menubar label inherits its color from the title bar",
  declares(bodyOf(menuRules, ".ws-window-titlebar__menu"), "color", "inherit"),
);
check(
  "the title bar sets the active label color the labels inherit",
  declares(bodyOf(chromeRules, ".ws-window-titlebar"), "color", "var(--titlebar-foreground)"),
);
const popoverBody = bodyOf(menuRules, ".ws-window-titlebar__popover");
check(
  "the menu popover has a 5px radius, an outline instead of a border, and scrolls",
  declares(popoverBody, "border-radius", "var(--ws-menu-radius)") &&
    /(?:^|[\s;])outline:\s*var\(--ws-border-width\) solid/.test(popoverBody) &&
    !/(?:^|[\s;])border:/.test(popoverBody) &&
    declares(popoverBody, "overflow-y", "auto"),
);
check("the menu radius token is 5px", /--ws-menu-radius:\s*var\(--ws-size-5\)/.test(await readFile(path.join(uiDir, "..", "src", "tokens", "component.css"), "utf8")));
check("a top-level dropdown has no fade", !/(?:^|[\s;])animation:/.test(popoverBody));
check(
  "flyouts and context menus keep the 83ms fade",
  declares(bodyOf(menuRules, ".ws-window-titlebar__popover--flyout"), "animation", "fadeIn 0.083s linear") &&
    declares(bodyOf(menuRules, ".ws-window-titlebar__popover--context"), "animation", "fadeIn 0.083s linear"),
);
check(
  "reduced motion drops the fade",
  /@media \(prefers-reduced-motion: reduce\)\s*\{[^@]*animation:\s*none/.test(
    stripComments(await readFile(path.join(uiDir, "..", "src", "parts", "menu", "window-menu.css"), "utf8")),
  ),
);

const itemBody = bodyOf(menuRules, ".ws-window-titlebar__item");
check("a row has no padding of its own", declares(itemBody, "padding", "0"));
check(
  "a row's hover and focus wash is #F0F0F01E, the active background",
  declares(bodyOf(menuRules, ".ws-window-titlebar__item:hover"), "background", "var(--cursor-bg-active)") &&
    declares(bodyOf(menuRules, ".ws-window-titlebar__item:focus-visible"), "background", "var(--cursor-bg-active)") &&
    tokenValue("--cursor-bg-active") === "#F0F0F01E",
);
check(
  "the label is padded 0 2em",
  declares(bodyOf(menuRules, ".ws-window-titlebar__item-label"), "padding", "0 2em"),
);
const shortcutBody = bodyOf(menuRules, ".ws-window-titlebar__shortcut");
check(
  "the shortcut is 13px, padded 0 2em, right-aligned, at 0.7 opacity",
  declares(shortcutBody, "font-size", "var(--font-size-base)") &&
    declares(shortcutBody, "padding", "0 2em") &&
    declares(shortcutBody, "margin-inline-start", "auto") &&
    declares(shortcutBody, "opacity", "0.7"),
);
check(
  "the shortcut is fully opaque on hover",
  declares(bodyOf(menuRules, ".ws-window-titlebar__item:hover .ws-window-titlebar__shortcut"), "opacity", "1"),
);
const checkBody = bodyOf(menuRules, ".ws-window-titlebar__item-check");
check(
  "the check mark sits in an absolute 2em left column, in the text color",
  declares(checkBody, "position", "absolute") &&
    declares(checkBody, "inset-inline-start", "0") &&
    declares(checkBody, "width", "2em") &&
    declares(checkBody, "color", "var(--text)"),
);
const chevronBody = bodyOf(menuRules, ".ws-window-titlebar__chevron");
check(
  "the submenu chevron is absolute, 9px from the right, at 0.7 opacity",
  declares(chevronBody, "position", "absolute") &&
    declares(chevronBody, "inset-inline-end", "var(--ws-size-9)") &&
    declares(chevronBody, "opacity", "0.7"),
);
const disabled = '.ws-window-titlebar__item[aria-disabled="true"]';
check(
  "a disabled row is half opaque with a #CCCCCC80 label and a 0.4 shortcut",
  declares(bodyOf(menuRules, disabled), "opacity", "0.5") &&
    declares(bodyOf(menuRules, `${disabled} .ws-window-titlebar__item-label`), "color", "var(--menu-disabled-label)") &&
    declares(bodyOf(menuRules, `${disabled} .ws-window-titlebar__shortcut`), "opacity", "0.4") &&
    tokenValue("--menu-disabled-label") === "#CCCCCC80",
);

if (failures.length > 0) {
  console.error(`titlebar-style: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("titlebar-style: all assertions passed");
