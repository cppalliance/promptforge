// Stylesheet contract for the Cursor IDE foundation: the token values in
// tokens.css, the controls in controls.css (button, input, select, the
// switch, the shared focus outline and menu scrollbar), the select menu in
// dropdown.css, the progress variants in progress.css, and the two dialog
// skins in modal.css. jsdom applies no stylesheets, so the values are read
// from the source text: each rule's declarations are collected by selector
// and every var() is resolved against the :root tokens, so a test names the
// value a reader sees (24px, #3FA266) instead of the token that carries it.
// Run: node test/skin-css.mjs (from crates/workshop/look).
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const flat = (text) => text.replace(/\s+/g, " ").trim();
const stripComments = (css) => css.replace(/\/\*[\s\S]*?\*\//g, "");

/** Every rule as { selectors, declarations }, nesting flattened (at-rule headers are dropped). */
function parseRules(css) {
  const rules = [];
  for (const match of stripComments(css).matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = match[1].split(",").map(flat);
    const declarations = {};
    for (const declaration of match[2].split(";")) {
      const colon = declaration.indexOf(":");
      if (colon < 0) continue;
      declarations[declaration.slice(0, colon).trim()] = flat(declaration.slice(colon + 1));
    }
    rules.push({ selectors, declarations });
  }
  return rules;
}

const read = (name) => readFile(path.join(lookDir, name), "utf8");
const tokensCss = await read("tokens.css");
const tokenRules = parseRules(tokensCss).filter((rule) => rule.selectors.includes(":root"));
const tokens = Object.assign({}, ...tokenRules.map((rule) => rule.declarations));

/** Resolves var() references, with fallbacks, against the :root tokens. */
function resolve(value, depth = 0) {
  if (value === undefined || depth > 12) return value;
  return value.replace(/var\(\s*(--[a-z0-9-]+)\s*(?:,\s*((?:[^()]|\([^()]*\))*))?\)/gi, (_, name, fallback) => {
    const known = tokens[name];
    if (known !== undefined) return resolve(known, depth + 1);
    return fallback === undefined ? "" : resolve(flat(fallback), depth + 1);
  });
}

/** Removes every `@media (prefers-reduced-motion: reduce) { ... }` block, which only turns motion off. */
function withoutReducedMotion(css) {
  const marker = "@media (prefers-reduced-motion: reduce)";
  let text = css;
  for (let start = text.indexOf(marker); start >= 0; start = text.indexOf(marker)) {
    const open = text.indexOf("{", start);
    let depth = 0;
    let end = open;
    for (; end < text.length; end += 1) {
      if (text[end] === "{") depth += 1;
      if (text[end] === "}") depth -= 1;
      if (depth === 0) break;
    }
    text = text.slice(0, start) + text.slice(end + 1);
  }
  return text;
}

function sheet(css) {
  const rules = parseRules(withoutReducedMotion(stripComments(css)));
  return {
    /** The merged declarations of every rule that lists `selector`, later rules winning. */
    of(selector) {
      const merged = {};
      for (const rule of rules) {
        if (rule.selectors.includes(selector)) Object.assign(merged, rule.declarations);
      }
      return merged;
    },
    has(selector) {
      return rules.some((rule) => rule.selectors.includes(selector));
    },
    /** The resolved value of one property on `selector`. */
    value(selector, property) {
      return resolve(this.of(selector)[property]) ?? "";
    },
  };
}

// --- tokens.css --------------------------------------------------------------------

const token = (name) => resolve(tokens[name]);
check("red is the IDE's #E34671", token("--cursor-red") === "#E34671");
check("blue is the IDE's #81A1C1", token("--cursor-blue") === "#81A1C1");
check("cyan is the IDE's #88C0D0", token("--cursor-cyan") === "#88C0D0");
check("the sidebar is #141414", token("--cursor-sidebar") === "#141414");
check("the active state background is #F0F0F01E", token("--cursor-bg-active") === "#F0F0F01E");
check("the focused state background is #F0F0F01E", token("--cursor-bg-focused") === "#F0F0F01E");
check("the primary shadow ink is #00000066", token("--cursor-shadow-primary") === "#00000066");
check(
  "the secondary shadow ink is 60% of the primary",
  token("--cursor-shadow-secondary") === "color-mix(in srgb, #00000066 60%, transparent)",
);
check(
  "the tertiary shadow ink is 30% of the primary",
  token("--cursor-shadow-tertiary") === "color-mix(in srgb, #00000066 30%, transparent)",
);
check(
  "the base box-shadow stack is the IDE's three layers",
  /0 0 0 1px .+, 0 0 4px 0(px)? .+, 0 8px 24px -2px /.test(tokens["--cursor-box-shadow-base"] ?? ""),
);
check(
  "the xl box-shadow stack is the IDE's four layers",
  /0 0 0 1px .+, 0 0 4px 0 .+, 0 12px 24px 0 .+, 0 24px 36px 0 /.test(tokens["--cursor-box-shadow-xl"] ?? ""),
);
check("the link color is #81A1C1", token("--cursor-text-link") === "#81A1C1");
check("the accent is the IDE's #81A1C1", token("--cursor-accent") === "#81A1C1");
check(
  "the accent hover mixes 10% white into the accent",
  token("--cursor-accent-hover") === "color-mix(in srgb, white 10%, #81A1C1)",
);
check(
  "the accent at 8% is its own token",
  token("--cursor-accent-subtle") === "color-mix(in srgb, #81A1C1 8%, transparent)",
);
check("the input field background is 3% of the base", token("--cursor-bg-input-field") === "color-mix(in srgb, #F0F0F0 3%, transparent)");
check("the inverted text is #181818", token("--cursor-text-invert") === "#181818");
check("the disabled control background is defined", (tokens["--cursor-control-disabled-bg"] ?? "") !== "");
check("the disabled control text is defined", (tokens["--cursor-control-disabled-text"] ?? "") !== "");
check("the extra-small radius is 2px", token("--radius-xs") === "2px");
check("the slower duration is 300ms", token("--duration-slower") === "300ms");
check(
  "tertiary text aliases the Cursor tertiary tier",
  tokens["--text-tertiary"] === "var(--cursor-text-tertiary)",
);
check(
  "quaternary text aliases the Cursor quaternary tier",
  tokens["--text-quaternary"] === "var(--cursor-text-quaternary)",
);
check(
  "tertiary is the brighter of the two tiers (60% over 36%)",
  token("--text-tertiary").includes("60%") && token("--text-quaternary").includes("36%"),
);
check(
  "the code font is Cursor's stack",
  tokens["--code-font"] === 'Consolas, Menlo, Monaco, "Droid Sans Mono", "Courier New", monospace',
);
check("the view scrollbar is 10px", token("--scrollbar-width") === "10px");
check("the menu scrollbar is 7px", token("--scrollbar-width-menu") === "7px");
check(
  "the focus outline is 1px at the focused stroke",
  token("--focus-outline") === "1px solid color-mix(in srgb, #F0F0F0 15%, transparent)",
);
check("the focus outline sits at offset -1px", token("--focus-outline-offset") === "-1px");

// --- controls.css ------------------------------------------------------------------

const controls = sheet(await read("controls.css"));

check(".button is 24px tall", controls.value(".button", "height") === "24px");
check(".button has 8px side padding", controls.value(".button", "padding-inline") === "8px");
check(".button has a 6px radius", controls.value(".button", "border-radius") === "6px");
check(".button is 13px", controls.value(".button", "font-size") === "13px");
check(".button is weight 400", controls.value(".button", "font-weight") === "400");
check(".button-sm is 20px tall", controls.value(".button-sm", "height") === "20px");
check(".button-sm has a 4px radius", controls.value(".button-sm", "border-radius") === "4px");
check(".button-xs is gone", !controls.has(".button-xs"));

check(".button-primary has #181818 text", controls.value(".button-primary", "color") === "#181818");
check(".button-primary is the accent, not gold", controls.value(".button-primary", "background") === "#81A1C1");
check(
  ".button-primary hover mixes 10% white into the accent",
  controls.value(".button-primary:hover", "background") === "color-mix(in srgb, white 10%, #81A1C1)",
);
check(
  ".button-secondary is an 8% fill",
  controls.value(".button-secondary", "background").includes("8%"),
);
check(
  ".button-secondary is 14% on hover",
  controls.value(".button-secondary:hover", "background").includes("14%"),
);
check(".button-ghost is transparent", controls.value(".button-ghost", "background") === "transparent");
check(
  ".button-outline is transparent with a 12% border",
  controls.value(".button-outline", "background") === "transparent" &&
    controls.value(".button-outline", "border-color").includes("12%"),
);
check(".button-danger exists", controls.has(".button-danger"));
check(
  ".button:disabled uses the disabled control colors",
  controls.value(".button:disabled", "background") === token("--cursor-control-disabled-bg") &&
    controls.value(".button:disabled", "color") === token("--cursor-control-disabled-text"),
);

check(".input is 24px tall", controls.value(".input", "height") === "24px");
check(".input pads 4px 5px", controls.value(".input", "padding") === "4px 5px");
check(".input has a 1px border at 12%", controls.value(".input", "border").startsWith("1px solid") && controls.value(".input", "border").includes("12%"));
check(".input has a 6px radius", controls.value(".input", "border-radius") === "6px");
check(".input has a 3% fill", controls.value(".input", "background").includes("3%"));
check(".input is 13px on an 18px line", controls.value(".input", "font-size") === "13px" && controls.value(".input", "line-height") === "18px");
check(".input transitions in 150ms", controls.value(".input", "transition").includes("150ms"));
check(".input border rises to 20% on focus", controls.value(".input:focus", "border-color").includes("20%"));

check(".select is 12px text", controls.value(".select", "font-size") === "12px");
check(".select has a 12% border", controls.value(".select", "border").includes("12%"));
check(".select has a 6px radius", controls.value(".select", "border-radius") === "6px");
check(".select pads 3px 2px 3px 6px", controls.value(".select", "padding") === "3px 2px 3px 6px");

check(".switch track is 32px wide", controls.value(".switch", "width") === "32px");
check(".switch track is 20px tall", controls.value(".switch", "height") === "20px");
check(".switch is 14% white when off", controls.value(".switch", "background").includes("14%"));
check(".switch is #3FA266 when on", controls.value('.switch[aria-checked="true"]', "background") === "#3FA266");
check(".switch thumb is 16px", controls.value(".switch::after", "width") === "16px" && controls.value(".switch::after", "height") === "16px");
check(".switch thumb is white", ["white", "#fff", "#FFFFFF"].includes(controls.value(".switch::after", "background")));
check(".switch thumb eases over 200ms", controls.value(".switch::after", "transition").includes("200ms"));
check(
  ".switch thumb moves 12px when on",
  controls.value('.switch[aria-checked="true"]::after', "transform") === "translateX(12px)",
);
check(".switch:disabled is dimmed", controls.has(".switch:disabled"));

const focusSelectors = [".input:focus-visible", ".select:focus-visible", ".list-row:focus-visible", ".tree-row:focus-visible"];
for (const selector of focusSelectors) {
  check(
    `${selector} draws the 1px outline at offset -1px`,
    controls.value(selector, "outline") === "1px solid color-mix(in srgb, #F0F0F0 15%, transparent)" &&
      controls.value(selector, "outline-offset") === "-1px",
  );
}

check(
  "menus use 7px scrollbars",
  controls.value(".scrollbar-menu::-webkit-scrollbar", "width") === "7px",
);

// --- dropdown.css ------------------------------------------------------------------

const dropdown = sheet(await read("dropdown.css"));
check("the select menu is #181818", dropdown.value(".menu-select", "background") === "#181818");
check("the select menu has a #F0F0F013 border", dropdown.value(".menu-select", "border").includes("#F0F0F013"));
check("the select menu has a 6px radius", dropdown.value(".menu-select", "border-radius") === "6px");
check("the select menu is at least 160px wide", dropdown.value(".menu-select", "min-width") === "160px");
check("select items pad 5px 8px", dropdown.value(".menu-select .menu-item", "padding") === "5px 8px");
check("select items rest at 0.6 opacity", dropdown.value(".menu-select .menu-item", "opacity") === "0.6");
check("select items are fully opaque on hover", dropdown.value(".menu-select .menu-item:hover", "opacity") === "1");
check(
  "menu rows draw the shared focus outline",
  dropdown.value(".menu-item:focus-visible", "outline") === "1px solid color-mix(in srgb, #F0F0F0 15%, transparent)" &&
    dropdown.value(".menu-item:focus-visible", "outline-offset") === "-1px",
);

// The composer menu surface: the mode, model, and @ menus.
check("the composer menu is #181818", dropdown.value(".menu-composer", "background") === "#181818");
check(
  "the composer menu has a 1px border at 15% of the base text color",
  dropdown.value(".menu-composer", "border") === "1px solid color-mix(in srgb, #F0F0F0 15%, transparent)",
);
check("the composer menu has a 6px radius", dropdown.value(".menu-composer", "border-radius") === "6px");
check("the composer menu pads 2px", dropdown.value(".menu-composer", "padding") === "2px");
check("the composer menu sets 12px text", dropdown.value(".menu-composer", "font-size") === "12px");
check(
  "composer rows pad 2px 6px with a 4px radius and 12px text",
  dropdown.value(".menu-composer .menu-item", "padding") === "2px 6px" &&
    dropdown.value(".menu-composer .menu-item", "border-radius") === "4px" &&
    dropdown.value(".menu-composer .menu-item", "font-size") === "12px",
);
check(
  "composer row icons are 14px",
  dropdown.value(".menu-composer .menu-item__icon svg", "width") === "14px" &&
    dropdown.value(".menu-composer .menu-item__icon svg", "height") === "14px",
);
check(
  "a composer row highlights #F0F0F011",
  dropdown.value(".menu-composer .menu-item:hover", "background") === "#F0F0F011",
);
check(
  "a selected composer row has no selected fill, only the check",
  dropdown.value(".menu-composer .menu-item--selected", "background") === "transparent",
);
check(
  "the composer check is 10px",
  dropdown.value(".menu-composer .menu-item__check svg", "width") === "10px" &&
    dropdown.value(".menu-composer .menu-item__check svg", "height") === "10px",
);
check(
  "a composer row's description reads in the tertiary tier at 11px",
  dropdown.value(".menu-composer .menu-item__description", "font-size") === "11px" &&
    dropdown.value(".menu-composer .menu-item__description", "color").includes("60%"),
);

// --- progress.css ------------------------------------------------------------------

const progress = sheet(await read("progress.css"));
check("the settings bar is 4px", progress.value(".progress", "height") === "4px");
check("the settings bar has a 2px radius", progress.value(".progress", "border-radius") === "2px");
check("the settings track is 12%", progress.value(".progress", "background").includes("12%"));
check("the settings fill eases its transform over 200ms", progress.value(".progress__fill", "transition").includes("200ms"));
check(
  "the indeterminate fill is a 34% bar sliding over 1.35s",
  progress.value(".progress--indeterminate .progress__fill", "width") === "34%" &&
    progress.value(".progress--indeterminate .progress__fill", "animation").includes("1.35s"),
);
check("the workbench bar is 2px", progress.value(".progress--workbench", "height") === "2px");
check("the workbench bar has square ends", progress.value(".progress--workbench", "border-radius") === "0");
check("the workbench fill is green", progress.value(".progress--workbench .progress__fill", "background") === "#3FA266");
check(
  "the workbench fill moves over 0.1s linear",
  progress.value(".progress--workbench .progress__fill", "transition").includes("0.1s linear"),
);

// --- modal.css ---------------------------------------------------------------------

const modal = sheet(await read("modal.css"));
check("the confirmation scrim is 0.5", modal.value(".modal-overlay--confirmation", "background") === "rgba(0,0,0,.5)");
check("the confirmation card sits 200px from the top", modal.value(".modal-overlay--confirmation", "padding-top") === "200px");
check(
  "the confirmation card is 300px to min(560px, 92vw)",
  modal.value(".modal-dialog--confirmation", "min-width") === "300px" &&
    modal.value(".modal-dialog--confirmation", "max-width") === "min(560px, 92vw)",
);
check("the confirmation card is #181818", modal.value(".modal-dialog--confirmation", "background") === "#181818");
check("the confirmation card has a 1px #F0F0F013 border", modal.value(".modal-dialog--confirmation", "border") === "1px solid #F0F0F013");
check("the confirmation card has an 8px radius", modal.value(".modal-dialog--confirmation", "border-radius") === "8px");
check(
  "the confirmation card has the 0 4px 20px shadow",
  modal.value(".modal-dialog--confirmation", "box-shadow") === "0 4px 20px rgba(0,0,0,.15)",
);
check("the confirmation card pads 12px", modal.value(".modal-dialog--confirmation", "padding") === "12px");
check("confirmation buttons sit 4px apart", modal.value(".modal-dialog--confirmation .modal-actions", "gap") === "4px");

check("the form card is 320px wide", modal.value(".modal-dialog--form", "width") === "320px");
check("the form card has a 12px radius", modal.value(".modal-dialog--form", "border-radius") === "12px");
check("the form card has a 12% stroke", modal.value(".modal-dialog--form", "border").includes("12%"));
check("the form card has no shadow", modal.value(".modal-dialog--form", "box-shadow") === "none");
check("the form backdrop is rgba(0,0,0,.5)", modal.value(".modal-overlay--form", "background") === "rgba(0,0,0,.5)");
check("the form card opens in 300ms", modal.value(".modal-dialog--form", "animation").includes("300ms"));
check(
  "the form card scales up from 0.97",
  stripCommentsIncludes(await read("modal.css"), "scale(0.97)"),
);
check("the form footer pads 10px", modal.value(".modal-dialog--form .modal-actions", "padding") === "10px");
check("the form footer has a top border at 8%", modal.value(".modal-dialog--form .modal-actions", "border-top").includes("8%"));

function stripCommentsIncludes(css, needle) {
  return stripComments(css).includes(needle);
}

// --- Reduced motion ------------------------------------------------------------------

/** The text of the sheet's `@media (prefers-reduced-motion: reduce)` blocks, or "". */
function reducedMotionBlocks(css) {
  const code = stripComments(css);
  return code.length - withoutReducedMotion(code).length > 0
    ? code.split("@media (prefers-reduced-motion: reduce)").slice(1).join("\n")
    : "";
}
const reduced = {
  controls: reducedMotionBlocks(await read("controls.css")),
  progress: reducedMotionBlocks(await read("progress.css")),
  modal: reducedMotionBlocks(await read("modal.css")),
};
check("the controls drop their transitions under reduced motion", /\.switch::after[\s\S]*transition:\s*none/.test(reduced.controls));
check("the progress bars drop their transitions and the slide under reduced motion", /transition:\s*none/.test(reduced.progress) && /animation:\s*none/.test(reduced.progress));
check("the form modal drops its open animation under reduced motion", /animation:\s*none/.test(reduced.modal));

if (failures.length > 0) {
  console.error(`skin-css: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("skin-css: all assertions passed");
