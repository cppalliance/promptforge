// Source-text contract for the quick input stylesheet
// (src/parts/quickinput/quick-input.css). jsdom applies no layout, so the
// values are read from the file and followed through the token sheets
// (test/helpers/css-values.mjs) down to the literals Cursor's workbench uses:
// the frame (width min(62vw, 820px), at most 80vw, an 8px radius, no border,
// the xl shadow, no padding or gap, 35px from the top), the rows (22px tall,
// padding 0 7px, margin 0 5px, a 6px radius, no gaps, a list at most
// min(440px, 40vh), selected #F0F0F01E, hover #F0F0F011), the box (26px, padding
// 0 7px, inside a 4px 6px header with no divider, placeholder #F0F0F099 at 0.5),
// the match highlights (#88C0D0 at weight 700, descriptions 0.9em at 0.6), the
// key chips (11px, 3px padding, a 3px radius, plain on the selected row), and
// the group labels (at the right of the row, a #F0F0F01C top line).
// Run: node test/quick-input-css.mjs
import { readUi, resolver, rulesOf, valueIn } from "./helpers/css-values.mjs";

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const resolve = await resolver();
const rules = rulesOf(await readUi("src/parts/quickinput/quick-input.css"));
const value = (selector, property) => resolve(valueIn(rules, selector, property));
const has = (selector) => rules.some((rule) => rule.selectors.includes(selector));

// --- The frame --------------------------------------------------------------------------

const FRAME = ".ws-quick-input";
check("the frame sits 35px from the top", value(FRAME, "top") === "35px");
check("the frame is min(62vw, 820px) wide", value(FRAME, "width") === "min(62vw, 820px)");
check("the frame is at most 80vw wide", value(FRAME, "max-width") === "80vw");
check("the frame has an 8px radius", value(FRAME, "border-radius") === "8px");
check("the frame has no border", value(FRAME, "border") === "none");
check(
  "the frame carries the IDE's xl shadow",
  value(FRAME, "box-shadow") === resolve("var(--cursor-box-shadow-xl)"),
);
check("the frame has no padding", value(FRAME, "padding") === "0");
check("the frame has no gap", value(FRAME, "gap") === "0");
check("the frame clips its rounded corners", value(FRAME, "overflow") === "hidden");

// --- The header and the box ---------------------------------------------------------------

check("the header pads 4px 6px", value(".ws-quick-input__header", "padding") === "4px 6px");
check("the header has no divider", value(".ws-quick-input__header", "border-bottom") === undefined || value(".ws-quick-input__header", "border-bottom") === "none");
const BOX = ".ws-quick-input__input";
check("the box is 26px tall", value(BOX, "height") === "26px");
check("the box pads 0 7px", value(BOX, "padding") === "0 7px");
check("the box has no border of its own", value(BOX, "border") === "none");
check("the placeholder is #F0F0F099", value(".ws-quick-input__input::placeholder", "color") === "#f0f0f099");
check("the placeholder is at half opacity", value(".ws-quick-input__input::placeholder", "opacity") === "0.5");

// --- The rows -----------------------------------------------------------------------------

const ROW = ".ws-quick-input__option";
check("a row is 22px tall", value(ROW, "height") === "22px");
check("a row pads 0 7px", value(ROW, "padding") === "0 7px");
check("a row keeps a 0 5px margin", value(ROW, "margin") === "0 5px");
check("a row has a 6px radius", value(ROW, "border-radius") === "6px");
check("a row has no gap", value(ROW, "gap") === "0");
check("the list has no gap between rows", value(".ws-quick-input__list", "gap") === "0");
check("the list is at most min(440px, 40vh) tall", value(".ws-quick-input__list", "max-height") === "min(440px, 40vh)");
check("the selected row is #F0F0F01E", value(`${ROW}[aria-selected="true"]`, "background") === "#f0f0f01e");
check("a hovered row is #F0F0F011", value(`${ROW}:hover`, "background") === "#f0f0f011");

// --- Matches and descriptions ----------------------------------------------------------------

check("a match highlight is #88C0D0", value(".ws-quick-input__highlight", "color") === "#88c0d0");
check("a match highlight is weight 700", value(".ws-quick-input__highlight", "font-weight") === "700");
check("a description is 0.9em", value(".ws-quick-input__option-description", "font-size") === "0.9em");
check("a description is at 0.6 opacity", value(".ws-quick-input__option-description", "opacity") === "0.6");

// --- Key chips ------------------------------------------------------------------------------------

const KEY = ".ws-quick-input__key";
check("a chip is 11px", value(KEY, "font-size") === "11px");
check("a chip pads 3px", value(KEY, "padding") === "3px");
check("a chip has a 3px radius", value(KEY, "border-radius") === "3px");
check("a chip draws a 1px solid border", /^1px solid /.test(value(KEY, "border") ?? ""));
check("a chord separator is 6px wide", value(".ws-quick-input__chord-separator", "width") === "6px");
check(
  "a chip is plain on the selected row",
  value(`${ROW}[aria-selected="true"] ${KEY}`, "background") === "none" &&
    value(`${ROW}[aria-selected="true"] ${KEY}`, "color") === "inherit",
);

// --- Group labels and the empty message ------------------------------------------------------------

const SEPARATOR = ".ws-quick-input__separator";
check("a group label sits at the right of its row", value(SEPARATOR, "justify-content") === "flex-end");
check("a group label draws a #F0F0F01C top line", /^1px solid #f0f0f01c$/.test(value(SEPARATOR, "border-top") ?? ""));
check("the first group label has no top line", value(`${SEPARATOR}:first-child`, "border-top") === "none");
check("a group label is 0.9em", value(SEPARATOR, "font-size") === "0.9em");
check("the empty message is a row-height line", value(".ws-quick-input__empty", "height") === "22px");
check("the empty message is dimmed to 0.6", value(".ws-quick-input__empty", "opacity") === "0.6");

// --- The hidden state, and no raw values -----------------------------------------------------------------

check("the stylesheet still declares the hidden state", has(".ws-quick-input[hidden]"));
const source = (await readUi("src/parts/quickinput/quick-input.css")).replace(/\/\*[\s\S]*?\*\//g, "");
const literals = source.match(/(?<![\w-])\d*\.?\d+(?:px|em|rem|vw|vh)\b|#[0-9a-fA-F]{3,8}\b|rgba?\(/g) ?? [];
check(`the stylesheet takes every size and color from a token (found ${literals.join(", ")})`, literals.length === 0);

if (failures.length > 0) {
  console.error(`quick-input-css: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("quick-input-css: all assertions passed");
