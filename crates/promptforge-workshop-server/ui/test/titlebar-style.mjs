// Title-bar style contract: asserts the custom Windows chrome skin against
// the visual spec, at the structural level (which variable feeds which
// property, which state gets which treatment) rather than pixel-peeping
// computed boxes. jsdom has no layout engine, so the DOM-side checks cover
// only what jsdom can prove: cascade behavior of the [hidden] rule and the
// Windows control order in the shipped markup.
// Run after `npm run build`: `node test/titlebar-style.mjs`.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "..", "dist");
const srcUiDir = path.join(uiDir, "..", "src", "ui");
// The title-bar rules live in per-component CSS files colocated with their
// owning modules; they ship bundled inside dist/app.css, where the other
// components' rules would confuse the first-match rule scan below, so this
// test reads the exact source files instead. Concatenation order mirrors the
// old single-file order: tokens (dist/style.css), chrome, menus, About dialog.
const [html, tokens, chrome, menu, about] = await Promise.all([
  readFile(path.join(distDir, "index.html"), "utf8"),
  readFile(path.join(distDir, "style.css"), "utf8"),
  readFile(path.join(srcUiDir, "window-chrome.css"), "utf8"),
  readFile(path.join(srcUiDir, "window-menu.css"), "utf8"),
  readFile(path.join(srcUiDir, "about-dialog.css"), "utf8"),
]);
const css = [tokens, chrome, menu, about].join("\n");

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Comments are stripped so a selector mentioned in prose can never be
// mistaken for a rule.
const cssText = css.replace(/\/\*[\s\S]*?\*\//g, "");

// Returns the declaration block of the first rule whose selector list
// contains `selector` exactly (a prefix like ".window-titlebar" must not
// match ".window-titlebar__icon"), or "" when no rule carries it.
function ruleBlock(selector) {
  let from = 0;
  for (;;) {
    const start = cssText.indexOf(selector, from);
    if (start === -1) return "";
    let i = start + selector.length;
    while (i < cssText.length && /\s/.test(cssText[i])) i += 1;
    if (cssText[i] === "{" || cssText[i] === ",") {
      const open = cssText.indexOf("{", i);
      const end = open === -1 ? -1 : cssText.indexOf("}", open);
      return end === -1 ? "" : cssText.slice(open + 1, end);
    }
    from = start + selector.length;
  }
}

// --- The :root skin block owns every themed value ---------------------------

const root = ruleBlock(":root");
check(":root skin block found", root !== "");
for (const variable of [
  "--titlebar-height",
  "--titlebar-control-width",
  "--titlebar-foreground",
  "--titlebar-hover",
  "--titlebar-divider",
  "--titlebar-accent",
]) {
  check(`:root declares ${variable}`, root.includes(`${variable}:`));
}

// --- The bar ----------------------------------------------------------------

const bar = ruleBlock(".window-titlebar");
check("the bar is a fixed height via --titlebar-height", /height:\s*var\(--titlebar-height,/.test(bar));
check("the bar paints the near-black surface", /background:\s*var\(--titlebar-bg,/.test(bar));
check(
  "the bar carries the one-pixel lower divider",
  /border-bottom:\s*1px solid var\(--titlebar-divider,/.test(bar),
);
check("the bar is not selectable", /user-select:\s*none/.test(bar));

const hidden = ruleBlock(".window-titlebar[hidden]");
check("the [hidden] rule hides the bar in browser mode", /display:\s*none/.test(hidden));

const drag = ruleBlock(".window-titlebar__drag");
check("the drag region is not selectable", /user-select:\s*none/.test(drag));

// --- Window controls ----------------------------------------------------------

const control = ruleBlock(".window-titlebar__control");
check("controls size from --titlebar-control-width", /width:\s*var\(--titlebar-control-width,/.test(control));
check("control hit areas keep a 24px floor", /min-height:\s*24px/.test(control));
check("control glyphs use the muted-gray token", /color:\s*var\(--titlebar-glyph,/.test(control));
check("controls are not selectable", /user-select:\s*none/.test(control));

const neutralHover = ruleBlock(".window-titlebar__control--minimize:hover");
check(
  "Minimize/Maximize hover is the neutral wash",
  /background:\s*var\(--titlebar-hover,/.test(neutralHover),
);

// The maximize/restore glyphs are SVG, outside the UA [hidden] rule's
// HTML-namespace scope, so the sheet must hide them itself.
const glyphHidden = ruleBlock(".window-titlebar__glyph--maximize[hidden]");
check("the glyph [hidden] rule hides the swapped-out SVG", /display:\s*none/.test(glyphHidden));

const closeHover = ruleBlock(".window-titlebar__control--close:hover");
check("Close hover is the red danger fill", /background:\s*var\(--titlebar-close-hover,/.test(closeHover));
check(
  "Close hover flips the glyph to the on-danger color",
  /color:\s*var\(--titlebar-close-glyph-hover,/.test(closeHover),
);

// --- Program icon and menu buttons ----------------------------------------------

const icon = ruleBlock(".window-titlebar__icon");
check("the program icon sizes from --titlebar-icon-size", /width:\s*var\(--titlebar-icon-size,/.test(icon));
check("the program icon height tracks the same token", /height:\s*var\(--titlebar-icon-size,/.test(icon));

const menuHover = ruleBlock(".window-titlebar__menu:hover");
check(
  "menu buttons take the neutral wash on hover and while open",
  /background:\s*var\(--titlebar-hover,/.test(menuHover),
);

// --- Menu popovers ------------------------------------------------------------

const popover = ruleBlock(".window-titlebar__popover");
check("popovers are raised near-black surfaces", /background:\s*var\(--bg-raised,/.test(popover));
check("popovers carry the subtle border", /border:\s*1px solid var\(--border,/.test(popover));
check(
  "popover width floors at --titlebar-popover-min-width",
  /min-width:\s*var\(--titlebar-popover-min-width,/.test(popover),
);
check(
  "popovers lift with the --titlebar-popover-shadow token",
  /box-shadow:\s*var\(--titlebar-popover-shadow,/.test(popover),
);

const itemFocus = ruleBlock(".window-titlebar__item:focus-visible");
check(
  "menu focus/selection ring uses the purple accent",
  /outline:\s*1px solid var\(--titlebar-accent,/.test(itemFocus),
);

const shortcut = ruleBlock(".window-titlebar__shortcut");
check("shortcut columns render muted", /color:\s*var\(--text-muted,/.test(shortcut));

const disabledHover = ruleBlock('.window-titlebar__item[aria-disabled="true"]:hover');
check("disabled menu rows suppress the hover wash", /background:\s*none/.test(disabledHover));

// --- Focus and motion discipline across the whole title-bar section -----------

const titlebarSection = cssText.slice(
  cssText.indexOf(".window-titlebar {"),
  cssText.indexOf(".about-dialog-overlay"),
);
check("no title-bar rule removes the focus outline", !/outline:\s*none/.test(titlebarSection));
check(
  "title-bar controls carry :focus-visible replacements",
  (titlebarSection.match(/:focus-visible/g) || []).length >= 3,
);
check(
  "the title bar animates nothing (hover states flip instantly)",
  !/(transition|animation)\s*:/.test(titlebarSection),
);

// --- What jsdom can prove about the shipped markup -----------------------------

const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
const { window } = dom;
const style = window.document.createElement("style");
style.textContent = css;
window.document.head.appendChild(style);

const barEl = window.document.querySelector(".window-titlebar");
check("title bar present in the shipped markup", barEl !== null);
if (barEl) {
  const order = [...barEl.querySelectorAll(".window-titlebar__control")].map((button) =>
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
    revealed.height.startsWith("var(--titlebar-height,"),
  );
  check(
    "--titlebar-height resolves to a fixed pixel value",
    /^\d+px$/.test(revealed.getPropertyValue("--titlebar-height").trim()),
  );
  const restoreGlyph = barEl.querySelector(".window-titlebar__glyph--restore");
  const maximizeGlyph = barEl.querySelector(".window-titlebar__glyph--maximize");
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
