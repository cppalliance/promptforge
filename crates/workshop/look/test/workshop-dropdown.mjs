// Unit test for the shared action menu (dropdown.ts), the floating menu
// the workshop's mode chip and model picker open. Bundles the module with
// esbuild and drives it against jsdom. Covers: opening renders a
// role=menu of menuitem buttons and wires the trigger's
// aria-haspopup/expanded/controls; activating an item runs its action,
// closes the menu, and restores the trigger's prior aria attributes; a
// danger item has the modifier class and an iconHtml renders inside
// the item; Escape closes and returns focus to the trigger; an outside
// pointer press closes while an inside press does not; showing from the
// same trigger toggles the menu closed and showing from another trigger
// swaps menus; ArrowDown/ArrowUp/Home/End move focus through the items
// with wrapping; dispose() closes an open menu so a disposed owner leaves
// no orphan in the DOM.
// Run: node test/workshop-dropdown.mjs (from crates/workshop/look).
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  entryPoints: [path.join(lookDir, "dropdown.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module imports its colocated CSS; the test drives only the JS,
  // and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});
const { DropdownMenu } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function menuEl() {
  return window.document.querySelector(".menu-popup");
}

function itemsOf(menu) {
  return [...menu.querySelectorAll(".menu-item")];
}

function pressKey(target, key) {
  target.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

function pointerDownOn(target) {
  target.dispatchEvent(new window.Event("pointerdown", { bubbles: true }));
}

const trigger = window.document.createElement("button");
trigger.type = "button";
trigger.textContent = "open";
window.document.body.appendChild(trigger);
const otherTrigger = window.document.createElement("button");
otherTrigger.type = "button";
window.document.body.appendChild(otherTrigger);

const dropdown = new DropdownMenu();

// --- Opening renders the menu and wires the trigger's aria state ------------

let firstClicks = 0;
let dangerClicks = 0;
dropdown.show(trigger, [
  { label: "First", iconHtml: "<svg></svg>", onClick: () => (firstClicks += 1) },
  { label: "Second", danger: true, onClick: () => (dangerClicks += 1) },
]);

{
  const menu = menuEl();
  check("showing appends one menu to the document", menu !== null);
  check("the menu has role=menu", menu?.getAttribute("role") === "menu");
  const items = itemsOf(menu);
  check("the menu renders one button per item", items.length === 2);
  check(
    "every item is a type=button menuitem",
    items.every(
      (item) =>
        item instanceof window.HTMLButtonElement &&
        item.type === "button" &&
        item.getAttribute("role") === "menuitem",
    ),
  );
  check("an item renders its label", items[0]?.textContent === "First");
  check(
    "an iconHtml renders inside the item's icon span",
    items[0]?.querySelector(".menu-item__icon svg") !== null,
  );
  check(
    "a danger item has the danger modifier class",
    items[1]?.classList.contains("menu-item--danger") === true &&
      items[0]?.classList.contains("menu-item--danger") === false,
  );
  check("the trigger gains aria-haspopup=menu", trigger.getAttribute("aria-haspopup") === "menu");
  check("the trigger gains aria-expanded=true", trigger.getAttribute("aria-expanded") === "true");
  check("the trigger's aria-controls names the menu", trigger.getAttribute("aria-controls") === menu?.id);
}

// --- Arrow keys, Home, and End move focus through the items -----------------

{
  const menu = menuEl();
  const items = itemsOf(menu);
  pressKey(menu, "ArrowDown");
  check("ArrowDown from the menu focuses the first item", window.document.activeElement === items[0]);
  pressKey(menu, "ArrowDown");
  check("ArrowDown advances to the next item", window.document.activeElement === items[1]);
  pressKey(menu, "ArrowDown");
  check("ArrowDown wraps from the last item to the first", window.document.activeElement === items[0]);
  pressKey(menu, "ArrowUp");
  check("ArrowUp wraps from the first item to the last", window.document.activeElement === items[1]);
  pressKey(menu, "Home");
  check("Home jumps to the first item", window.document.activeElement === items[0]);
  pressKey(menu, "End");
  check("End jumps to the last item", window.document.activeElement === items[1]);
}

// --- Activating an item runs its action and closes the menu -----------------

{
  itemsOf(menuEl())[0].click();
  check("clicking an item runs its onClick once", firstClicks === 1 && dangerClicks === 0);
  check("clicking an item closes the menu", menuEl() === null);
  check("closing removes aria-haspopup from the trigger", trigger.getAttribute("aria-haspopup") === null);
  check("closing removes aria-expanded from the trigger", trigger.getAttribute("aria-expanded") === null);
  check("closing removes aria-controls from the trigger", trigger.getAttribute("aria-controls") === null);
}

// --- Escape closes and returns focus to the trigger --------------------------

{
  dropdown.show(trigger, [{ label: "Only", onClick: () => undefined }]);
  check("the menu reopens for the Escape case", menuEl() !== null);
  pressKey(window.document, "Escape");
  check("Escape closes the menu", menuEl() === null);
  check("Escape returns focus to the trigger", window.document.activeElement === trigger);
}

// --- Outside pointer presses close; inside presses do not --------------------

{
  dropdown.show(trigger, [{ label: "Only", onClick: () => undefined }]);
  pointerDownOn(itemsOf(menuEl())[0]);
  check("a pointer press inside the menu keeps it open", menuEl() !== null);
  pointerDownOn(trigger);
  check("a pointer press on the trigger keeps the menu open", menuEl() !== null);
  pointerDownOn(window.document.body);
  check("a pointer press outside the menu closes it", menuEl() === null);
}

// --- Same-trigger show toggles closed; another trigger swaps menus -----------

{
  dropdown.show(trigger, [{ label: "Only", onClick: () => undefined }]);
  const first = menuEl();
  dropdown.show(trigger, [{ label: "Only", onClick: () => undefined }]);
  check("showing again from the same trigger toggles the menu closed", menuEl() === null);
  check("the toggled-away menu left the document", first?.isConnected === false);

  dropdown.show(trigger, [{ label: "Only", onClick: () => undefined }]);
  const before = menuEl();
  dropdown.show(otherTrigger, [{ label: "Other", onClick: () => undefined }]);
  const after = menuEl();
  check(
    "showing from another trigger swaps to a new menu",
    before?.isConnected === false && after !== null && after !== before,
  );
  check(
    "the swap moves the aria wiring to the new trigger",
    trigger.getAttribute("aria-expanded") === null && otherTrigger.getAttribute("aria-expanded") === "true",
  );
  check(
    "one menu at a time",
    window.document.querySelectorAll(".menu-popup").length === 1,
  );
}

// --- The composer skin: check-only selection, descriptions, placement --------

{
  const anchored = window.document.createElement("button");
  anchored.type = "button";
  window.document.body.appendChild(anchored);
  // jsdom has no layout, so the trigger's rect and the menu's measured size are stubbed.
  anchored.getBoundingClientRect = () => ({
    x: 40, y: 200, left: 40, right: 100, top: 200, bottom: 224, width: 60, height: 24, toJSON: () => ({}),
  });
  const probe = new DropdownMenu();
  probe.show(
    anchored,
    [
      { label: "Agent", description: "Plan, search, build anything", selected: true, onClick: () => undefined },
      { label: "Plan", onClick: () => undefined },
    ],
    undefined,
    { skin: "composer", className: "ws-test-menu", placement: "above", offsetX: -6 },
  );
  // The earlier menu from `dropdown` is still open, so the probe's is the last one.
  const menu = [...window.document.querySelectorAll(".menu-popup")].at(-1);
  const rows = itemsOf(menu);
  check(
    "the composer skin adds its classes beside the shared ones",
    menu?.classList.contains("menu") === true &&
      menu.classList.contains("menu-popup") &&
      menu.classList.contains("menu-composer") &&
      menu.classList.contains("scrollbar-menu"),
  );
  check("an options className lands on the menu", menu?.classList.contains("ws-test-menu") === true);
  check(
    "a description sits under its label in one text column",
    rows[0]?.querySelector(".menu-item__text > .menu-item__label")?.textContent === "Agent" &&
      rows[0]?.querySelector(".menu-item__text > .menu-item__description")?.textContent ===
        "Plan, search, build anything",
  );
  check(
    "a row without a description keeps the bare label",
    rows[1]?.querySelector(".menu-item__text") === null &&
      rows[1]?.querySelector(":scope > .menu-item__label")?.textContent === "Plan",
  );
  check(
    "the selected row's check is a codicon, not a text glyph",
    rows[0]?.querySelector(".menu-item__check svg") !== null &&
      rows[0]?.querySelector(".menu-item__check")?.textContent === "",
  );
  check(
    "an above placement with an offset opens at the trigger's left minus 6px",
    menu?.style.left === "34px" && menu?.style.right === "auto",
  );
  check(
    "an above placement sits over the trigger, not under it",
    Number.parseFloat(menu?.style.top ?? "999") < 200,
  );
  probe.dispose();
  anchored.remove();
}

{
  // The plain skin is untouched: the text check, no composer classes.
  const plain = new DropdownMenu();
  plain.show(trigger, [{ label: "One", selected: true, onClick: () => undefined }]);
  const last = [...window.document.querySelectorAll(".menu-popup")].at(-1);
  check(
    "the default skin keeps the text check and no composer class",
    last?.classList.contains("menu-composer") === false &&
      itemsOf(last)[0]?.querySelector(".menu-item__check")?.textContent === "✓",
  );
  plain.dispose();
}

// --- dispose() closes the open menu ------------------------------------------

{
  check("a menu is open before dispose", menuEl() !== null);
  dropdown.dispose();
  check("dispose closes the open menu", menuEl() === null);
  check("dispose restores the trigger's aria state", otherTrigger.getAttribute("aria-expanded") === null);
}

if (failures.length > 0) {
  console.error(`workshop-dropdown: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-dropdown: all assertions passed");
