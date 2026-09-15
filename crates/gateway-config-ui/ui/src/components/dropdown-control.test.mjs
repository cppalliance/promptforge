// Pins the select control's grouped and greyed rows: a group header
// renders once per group change and is neither a row nor focusable;
// a greyed row is never chosen by click or Enter; arrow, Home, End,
// and typeahead navigation skip greyed rows and the initial focus lands
// on an enabled one; a caller passing neither field gets the old
// structure (rows only, no headers, nothing greyed).
import assert from "node:assert/strict";
import test from "node:test";

import { loadApp, makeDom } from "../harness.mjs";

const app = await loadApp();

/** Mounts a control into a fresh document and returns its handles. */
function mount(options, value = "", onChange = () => {}) {
  const dom = makeDom();
  const changes = [];
  const control = app.createDropdownControl({
    id: "pick",
    options,
    value,
    onChange: (next) => {
      changes.push(next);
      onChange(next);
    },
  });
  dom.window.document.body.append(control.element);
  const rows = [...control.element.querySelectorAll(".menu-item")];
  const headers = [...control.element.querySelectorAll(".menu-group-label")];
  return { dom, control, rows, headers, changes };
}

const key = (dom, target, keyName) =>
  target.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: keyName, bubbles: true }));

const GROUPED = [
  { value: "a", label: "Alpha", group: "Prime" },
  { value: "b", label: "Bravo", group: "Prime", disabled: true },
  { value: "c", label: "Charlie", group: "Subprime" },
  { value: "d", label: "Delta", group: "Niche", disabled: true },
  { value: "e", label: "Echo", group: "Niche" },
];

test("a group header renders once per group change and is not a row", () => {
  const { control, rows, headers } = mount(GROUPED, "a");
  assert.deepEqual(
    headers.map((header) => header.textContent),
    ["Prime", "Subprime", "Niche"],
    "consecutive rows sharing a group share one header",
  );
  assert.equal(rows.length, GROUPED.length, "headers are not option rows");
  const children = [...control.element.querySelector(".menu").children];
  assert.deepEqual(
    children.map((child) => child.className),
    [
      "menu-group-label",
      "menu-item",
      "menu-item",
      "menu-group-label",
      "menu-item",
      "menu-group-label",
      "menu-item",
      "menu-item",
    ],
    "each header precedes the first row of its group",
  );
  for (const header of headers) {
    assert.equal(header.getAttribute("role"), "presentation");
    assert.equal(header.tagName, "DIV", "a header is not a button");
    assert.equal(header.tabIndex, -1, "a header is not focusable");
    assert.equal(header.getAttribute("aria-selected"), null);
  }
});

test("a greyed row carries disabled and aria-disabled and is never chosen", () => {
  const { dom, control, rows, changes } = mount(GROUPED, "a");
  const greyed = rows[1];
  assert.equal(greyed.disabled, true);
  assert.equal(greyed.getAttribute("aria-disabled"), "true");
  assert.equal(rows[0].hasAttribute("aria-disabled"), false, "an enabled row carries no flag");
  control.trigger.click();
  greyed.click();
  key(dom, greyed, "Enter");
  key(dom, greyed, " ");
  assert.deepEqual(changes, [], "click, Enter, and Space on a greyed row fire nothing");
  assert.equal(control.trigger.value, "a", "the selection stays put");
  assert.equal(control.element.querySelector(".menu").hidden, false, "the listbox stays open");
  rows[2].click();
  assert.deepEqual(changes, ["c"], "an enabled row still chooses");
  assert.equal(control.trigger.textContent, "Charlie");
});

test("arrow, Home, End, and typeahead navigation skip greyed rows", () => {
  const { dom, control, rows } = mount(GROUPED, "a");
  const doc = dom.window.document;
  control.trigger.click();
  assert.equal(doc.activeElement, rows[0], "the listbox opens on the selection");
  key(dom, rows[0], "ArrowDown");
  assert.equal(doc.activeElement, rows[2], "ArrowDown skips the greyed Bravo");
  key(dom, rows[2], "ArrowDown");
  assert.equal(doc.activeElement, rows[4], "ArrowDown skips the greyed Delta");
  key(dom, rows[4], "ArrowDown");
  assert.equal(doc.activeElement, rows[0], "ArrowDown wraps to the first enabled row");
  key(dom, rows[0], "ArrowUp");
  assert.equal(doc.activeElement, rows[4], "ArrowUp wraps past the end");
  key(dom, rows[4], "ArrowUp");
  assert.equal(doc.activeElement, rows[2], "ArrowUp skips the greyed Delta");
  key(dom, rows[2], "Home");
  assert.equal(doc.activeElement, rows[0]);
  key(dom, rows[0], "End");
  assert.equal(doc.activeElement, rows[4], "End lands on the last enabled row");
  key(dom, rows[4], "d");
  assert.equal(doc.activeElement, rows[4], "typeahead does not land on the greyed Delta");
  key(dom, rows[4], "Escape");
  key(dom, rows[4], "c");
  assert.equal(doc.activeElement, control.trigger, "Escape returned focus to the trigger");
});

test("the initial focus and the trigger's End key land on enabled rows", () => {
  const greyedFirstAndLast = [
    { value: "x", label: "X", disabled: true },
    { value: "y", label: "Y" },
    { value: "z", label: "Z", disabled: true },
  ];
  const { dom, control, rows } = mount(greyedFirstAndLast, "x");
  const doc = dom.window.document;
  control.trigger.click();
  assert.equal(doc.activeElement, rows[1], "a greyed selection opens on the first enabled row");
  control.trigger.click();
  key(dom, control.trigger, "End");
  assert.equal(doc.activeElement, rows[1], "End from the trigger skips the greyed last row");
  key(dom, rows[1], "Escape");
  key(dom, control.trigger, "Home");
  assert.equal(doc.activeElement, rows[1], "Home from the trigger skips the greyed first row");
});

test("a caller passing neither group nor disabled gets the old structure", () => {
  const plain = [
    { value: "", label: "None" },
    { value: "one", label: "One" },
    { value: "two", label: "Two" },
  ];
  const { dom, control, rows, headers, changes } = mount(plain, "one");
  assert.equal(headers.length, 0, "no headers");
  assert.deepEqual(
    [...control.element.querySelector(".menu").children].map((child) => child.className),
    ["menu-item", "menu-item", "menu-item"],
  );
  for (const row of rows) {
    assert.equal(row.disabled, false);
    assert.equal(row.hasAttribute("aria-disabled"), false);
  }
  assert.equal(rows[1].getAttribute("aria-selected"), "true");
  assert.equal(control.trigger.textContent, "One");
  control.trigger.click();
  assert.equal(dom.window.document.activeElement, rows[1]);
  key(dom, rows[1], "ArrowDown");
  key(dom, rows[2], "ArrowDown");
  assert.equal(dom.window.document.activeElement, rows[0], "plain wrap-around is unchanged");
  rows[0].click();
  assert.deepEqual(changes, [""]);
  control.setValue("two");
  assert.equal(control.trigger.textContent, "Two", "setValue still moves without firing");
  assert.deepEqual(changes, [""]);
});
