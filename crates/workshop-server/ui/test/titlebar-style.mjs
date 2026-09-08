// Title-bar built-artifact contract: loads the bundled stylesheet and
// shipped markup into jsdom, then checks the DOM, visibility, sizing, glyph,
// and keyboard-focus behavior that jsdom can execute without a layout
// engine.
// Run after `npm run build`: `node test/titlebar-style.mjs`.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "..", "dist");
const [html, css] = await Promise.all([
  readFile(path.join(distDir, "index.html"), "utf8"),
  readFile(path.join(distDir, "app.css"), "utf8"),
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
