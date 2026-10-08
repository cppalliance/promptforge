// Source-text contract for the Cursor IDE foundation in the Workshop UI's own
// stylesheets (jsdom applies no stylesheets, so the values are read from the
// files): the global reset no longer strips focus outlines, so the shared
// look rules draw Cursor's; the global scrollbars are the view bars (the
// look token's width, square, shown while the scroller is hovered), and a
// menu marked `.scrollbar-menu` keeps look's 7px bar through those unlayered
// view rules; and the per-dialog copies of the modal skin are gone from the
// layout and editor stylesheets, because the shared confirmation and form
// skins own the look of the Add Folder, save, revert, and overwrite prompts.
// Run: node test/foundation-css.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relative) => readFile(path.join(uiDir, relative), "utf8");
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, "");

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** Every rule as { selectors, body }, nesting flattened. */
function rulesOf(css) {
  return [...stripComments(css).matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
    selectors: match[1].split(",").map((selector) => selector.replace(/\s+/g, " ").trim()),
    body: match[2].replace(/\s+/g, " ").trim(),
  }));
}

// --- style.css -----------------------------------------------------------------------

const style = rulesOf(await read("style.css"));
check(
  "no global rule strips focus outlines",
  !style.some((rule) => rule.selectors.includes(":focus") && /outline:\s*none/.test(rule.body)),
);
check(
  "no style.css rule forces an outline off with !important",
  !style.some((rule) => /outline:\s*none\s*!important/.test(rule.body)),
);

const thumb = style.find((rule) => rule.selectors.includes("::-webkit-scrollbar-thumb"));
check("the scrollbar thumb is square", /border-radius:\s*0\b/.test(thumb?.body ?? ""));
check(
  "the scrollbar thumb is hidden until the scroller is hovered",
  /background:\s*transparent/.test(thumb?.body ?? ""),
);
check(
  "a hovered scroller shows the thumb",
  style.some(
    (rule) =>
      rule.selectors.some((selector) => /:hover::-webkit-scrollbar-thumb$/.test(selector)) &&
      rule.body.includes("var(--scrollbar-thumb)"),
  ),
);
const bar = style.find((rule) => rule.selectors.includes("::-webkit-scrollbar"));
check(
  "the view scrollbar takes its width from the look token",
  (bar?.body ?? "").includes("var(--scrollbar-width)"),
);

// --- a menu keeps its 7px bar through the view rules ----------------------------------

/**
 * Every style rule with whether it sits inside an `@layer` block. At-rule wrappers (`@layer`,
 * `@supports`, `@media`) are flattened; a rule's own header is its selector list.
 */
function layeredRules(css) {
  const code = stripComments(css);
  const rules = [];
  const open = [];
  let headerStart = 0;
  for (let i = 0; i < code.length; i += 1) {
    if (code[i] === "{") {
      open.push({ header: code.slice(headerStart, i).replace(/\s+/g, " ").trim(), bodyStart: i + 1 });
      headerStart = i + 1;
    } else if (code[i] === "}") {
      const closed = open.pop();
      const body = code.slice(closed.bodyStart, i);
      if (!closed.header.startsWith("@") && !body.includes("{")) {
        rules.push({
          selectors: closed.header.split(",").map((selector) => selector.trim()),
          body: body.replace(/\s+/g, " ").trim(),
          layered: open.some((entry) => entry.header.startsWith("@layer")),
        });
      }
      headerStart = i + 1;
    } else if (code[i] === ";") {
      headerStart = i + 1;
    }
  }
  return rules;
}

const declaration = (body, property) => {
  for (const part of body.split(";")) {
    const colon = part.indexOf(":");
    if (colon >= 0 && part.slice(0, colon).trim() === property) return part.slice(colon + 1).trim();
  }
  return undefined;
};

/**
 * The value the cascade gives a scrollbar pseudo-element property: an unlayered rule beats a
 * layered one whatever the specificity, then the more specific selector wins, then the later rule.
 * Models only the selector shapes the scrollbar rules use:
 * `[.class][:hover]::pseudo[:hover]`, where a leading `:hover` is the scroller and a trailing one the part.
 */
function cascaded(rules, element, pseudo, property) {
  let best = null;
  rules.forEach((rule, order) => {
    const value = declaration(rule.body, property);
    if (value === undefined) return;
    for (const selector of rule.selectors) {
      const parts = /^(?:\.([\w-]+))?(:hover)?(::[\w-]+)(:hover)?$/.exec(selector);
      if (parts === null || parts[3] !== pseudo) continue;
      const [, cls, scrollerHover, , partHover] = parts;
      if (cls !== undefined && !element.classes.includes(cls)) continue;
      if (scrollerHover !== undefined && !element.scrollerHover) continue;
      if (partHover !== undefined && !element.partHover) continue;
      const rank = [
        rule.layered ? 0 : 1,
        (cls === undefined ? 0 : 1) + (scrollerHover === undefined ? 0 : 1) + (partHover === undefined ? 0 : 1),
        order,
      ];
      const beats =
        best === null ||
        rank[0] > best.rank[0] ||
        (rank[0] === best.rank[0] && (rank[1] > best.rank[1] || (rank[1] === best.rank[1] && rank[2] > best.rank[2])));
      if (beats) best = { rank, value };
    }
  });
  return best?.value;
}

const controlsPath = fileURLToPath(import.meta.resolve("@workshop/look/controls.css"));
const controlRules = layeredRules(await readFile(controlsPath, "utf8"));
const viewRules = layeredRules(await read("style.css"));
// Bundle order: look's sheets first, then the app's base sheet.
const scrollbarRules = [...controlRules, ...viewRules];

const menuRules = controlRules.filter((rule) => rule.selectors.some((selector) => selector.startsWith(".scrollbar-menu")));
check("look has the menu scrollbar rules", menuRules.length > 0);
check(
  "no menu scrollbar rule sits in a layer, where the unlayered view rules would beat it",
  menuRules.every((rule) => !rule.layered),
);
check(
  "style.css's view scrollbar rules are unlayered, which is why the menu rules must be too",
  viewRules.filter((rule) => rule.selectors.includes("::-webkit-scrollbar")).every((rule) => !rule.layered),
);

const menu = { classes: ["scrollbar-menu"], scrollerHover: false, partHover: false };
const view = { classes: [], scrollerHover: false, partHover: false };
check(
  "a menu scrollbar is 7px wide after the view rules",
  cascaded(scrollbarRules, menu, "::-webkit-scrollbar", "width") === "var(--scrollbar-width-menu)",
);
check(
  "a menu scrollbar is 7px tall after the view rules",
  cascaded(scrollbarRules, menu, "::-webkit-scrollbar", "height") === "var(--scrollbar-width-menu)",
);
check(
  "a view scrollbar stays 10px wide",
  cascaded(scrollbarRules, view, "::-webkit-scrollbar", "width") === "var(--scrollbar-width)",
);
check(
  "a menu thumb shows without hover, unlike the view thumb",
  cascaded(scrollbarRules, menu, "::-webkit-scrollbar-thumb", "background") === "var(--scrollbar-thumb)" &&
    cascaded(scrollbarRules, view, "::-webkit-scrollbar-thumb", "background") === "transparent",
);
check(
  "a hovered menu thumb takes the hover color",
  cascaded(scrollbarRules, { ...menu, partHover: true }, "::-webkit-scrollbar-thumb", "background") ===
    "var(--scrollbar-thumb-hover)",
);

// --- the per-dialog copies are gone --------------------------------------------------

const zones = rulesOf(await read("src/parts/layout/zones.css"));
check(
  "zones.css carries no Add Folder dialog rules",
  !zones.some((rule) => rule.selectors.some((selector) => selector.includes("ws-workspace-add"))),
);
const editor = rulesOf(await read("src/parts/editor/editor-panel.css"));
check(
  "editor-panel.css carries no panel dialog rules",
  !editor.some((rule) =>
    rule.selectors.some((selector) => /ws-editor-(conflict|close|revert)/.test(selector)),
  ),
);
check(
  "editor-panel.css keeps the panel and error bar rules",
  editor.some((rule) => rule.selectors.includes(".ws-editor-panel")) &&
    editor.some((rule) => rule.selectors.includes(".ws-editor-panel__error")),
);

const component = await read("src/tokens/component.css");
check(
  "the dialog size tokens left with their stylesheets",
  !component.includes("--ws-editor-dialog-max-width") && !component.includes("--ws-workspace-dialog-min-width"),
);

if (failures.length > 0) {
  console.error(`foundation-css: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("foundation-css: all assertions passed");
