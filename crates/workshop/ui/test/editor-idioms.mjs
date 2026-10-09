// Editor idiom test (step 16, src/parts/editor/editor-surface.ts): the
// readOnly toggle runs through a Compartment - one reconfigure dispatch,
// so document text, dirty tracking, and the live view all survive a
// toggle - and reloads into a live view dispatch a transaction tagged
// with the externalUpdate annotation, distinguishable from local typing
// via the exported isExternalUpdate helper. Runs CodeMirror under jsdom
// with the same measurement shims as editor-panel.mjs.
// Run: node test/editor-idioms.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";
import { declaration, readUi, resolver, rulesOf } from "./helpers/css-values.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "@workshop/platform/lifecycle";
      export {
        CodeMirrorSurface,
        externalUpdate,
        isExternalUpdate,
      } from "./src/parts/editor/editor-surface.ts";
      export { redo, undo, undoDepth } from "@codemirror/commands";
      export { EditorView, getDrawSelectionConfig } from "@codemirror/view";
      export { foldEffect, getIndentUnit, indentUnit } from "@codemirror/language";
      export { closeSearchPanel, getSearchQuery, openSearchPanel, searchPanelOpen } from "@codemirror/search";
      export { revealReplace } from "./src/parts/editor/find-widget.ts";
      export { ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT } from "@workshop/look/icons";
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
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7911/",
  pretendToBeVisual: true,
});
const { window } = dom;

// CodeMirror measures text through Range, which jsdom does not layout;
// zero-rect shims are enough because the test never asserts geometry.
const zeroRect = () => ({
  x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0,
  toJSON: () => ({}),
});
window.Range.prototype.getBoundingClientRect = zeroRect;
window.Range.prototype.getClientRects = () => ({
  length: 0,
  item: () => null,
  [Symbol.iterator]: [][Symbol.iterator],
});
window.HTMLElement.prototype.getClientRects = function getClientRects() {
  return { length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] };
};
if (!window.HTMLElement.prototype.getBoundingClientRect) {
  window.HTMLElement.prototype.getBoundingClientRect = zeroRect;
}
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

for (const key of [
  "document",
  "navigator",
  "Window",
  "HTMLElement",
  "HTMLInputElement",
  "HTMLButtonElement",
  "KeyboardEvent",
  "Node",
  "Element",
  "Range",
  "Event",
  "CustomEvent",
  "MutationObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;

const bundlePath = path.join(os.tmpdir(), "promptforge-editor-idioms-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  lifecycle,
  CodeMirrorSurface,
  externalUpdate,
  isExternalUpdate,
  redo,
  undo,
  undoDepth,
  EditorView,
  getDrawSelectionConfig,
  foldEffect,
  getIndentUnit,
  indentUnit,
  closeSearchPanel,
  getSearchQuery,
  openSearchPanel,
  searchPanelOpen,
  revealReplace,
  ICON_CHEVRON_DOWN,
  ICON_CHEVRON_RIGHT,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const FILE_PATH = "C:\\project\\notes.md";

function contentEditableOf(surface) {
  return surface.element.querySelector(".cm-content")?.getAttribute("contenteditable");
}

await assertNoLeaks(lifecycle, async () => {
  // --- readOnly toggles through the compartment ----------------------------

  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: FILE_PATH, text: "alpha\n" });

  check("a fresh surface accepts input", contentEditableOf(surface) === "true");
  check("a fresh surface state is not readOnly", surface.view.state.readOnly === false);

  surface.setReadOnly(true);
  check("setReadOnly(true) flips the state's readOnly facet", surface.view.state.readOnly === true);
  check("setReadOnly(true) makes the content non-editable", contentEditableOf(surface) === "false");

  surface.setReadOnly(false);
  check(
    "setReadOnly(false) restores the editable state",
    surface.view.state.readOnly === false && contentEditableOf(surface) === "true",
  );

  // --- toggling reconfigures in place: no state rebuild ---------------------

  // The view is TypeScript-private but reachable at runtime, same as in
  // editor-panel.mjs: the assertions drive the real dispatch path.
  surface.view.dispatch({ changes: { from: 0, insert: "x" } });
  check("an edit before the toggle dirties the surface", surface.isDirty());

  const viewBefore = surface.view;
  const editorNodeBefore = surface.element.querySelector(".cm-editor");
  surface.setReadOnly(true);
  surface.setReadOnly(false);
  check(
    "toggling keeps the same view and editor DOM node - no rebuild",
    surface.view === viewBefore &&
      surface.element.querySelector(".cm-editor") === editorNodeBefore,
  );
  check("toggling keeps the document text", surface.text() === "xalpha\n");
  check("toggling keeps the dirty state", surface.isDirty());

  // The unrelated dirty-tracking extension survives the toggles: reverting
  // the edit still lands on the updateListener and clears dirty.
  surface.view.dispatch({ changes: { from: 0, to: 1 } });
  check("the dirty updateListener still runs after toggles", !surface.isDirty());

  // --- externalUpdate tags server-originated reloads ------------------------

  // Capture the transactions the surface dispatches by intercepting the
  // view's update entry point; open() keeps the view alive on reload, so
  // the interception survives it.
  const captured = [];
  const view = surface.view;
  const realUpdate = view.update.bind(view);
  view.update = (transactions) => {
    captured.push(...transactions);
    realUpdate(transactions);
  };

  surface.open({ path: FILE_PATH, text: "from the server\n" });
  check(
    "a reload into a live view dispatches a transaction tagged externalUpdate",
    captured.some((tr) => isExternalUpdate(tr)),
  );
  check(
    "the annotation itself reads back off the transaction",
    captured.some((tr) => tr.annotation(externalUpdate) === true),
  );
  check("the reload replaced the document text", surface.text() === "from the server\n");
  check("a reload is not dirty", !surface.isDirty());

  // The reload deliberately enters undo history (plan step 16, decision 3):
  // undo can cross it back into pre-reload text, and dirty tracking flags
  // the result against the reloaded baseline.
  check("the reload keeps undo history", undoDepth(view.state) > 0);
  undo(view);
  check("undo crosses the reload into the pre-reload text", surface.text() === "alpha\n");
  check("undoing past the reload marks the surface dirty", surface.isDirty());
  redo(view);
  check(
    "redo reapplies the reload and returns to clean",
    surface.text() === "from the server\n" && !surface.isDirty(),
  );

  const capturedBefore = captured.length;
  view.dispatch({ changes: { from: 0, insert: "typed " } });
  check(
    "a typed change is not tagged externalUpdate",
    captured.length === capturedBefore + 1 && !isExternalUpdate(captured.at(-1)),
  );
  check("the typed change landed", surface.text() === "typed from the server\n");

  surface.dispose();
  check("dispose still tears the editor down", !surface.element.querySelector(".cm-editor"));

  // --- readOnly set before open() seeds the first state ---------------------

  const lockedSurface = new CodeMirrorSurface();
  window.document.body.appendChild(lockedSurface.element);
  lockedSurface.setReadOnly(true);
  lockedSurface.open({ path: "C:\\project\\b.txt", text: "locked\n" });
  check(
    "readOnly set before open applies to the first state",
    lockedSurface.view.state.readOnly === true && contentEditableOf(lockedSurface) === "false",
  );
  lockedSurface.dispose();
});

// --- Cursor's editor idioms ---------------------------------------------------
//
// The behaviors and look the surface copies from Cursor: a has-selection
// class that hides the current line, Monaco's cursor blink, indent unit,
// scroll-past-end, Alt+click and Shift+Alt+drag gestures (no crosshair), a
// glyph lane before the line numbers, fold chevrons, and a floating find
// widget. The look is read back from the stylesheets the editor mounts
// (jsdom applies no layout, so the declared values are the contract): the
// theme's rules from the document's injected <style> text, resolved through
// the token sheets down to Cursor Dark's literals.

const resolve = await resolver();
const panelRules = rulesOf(await readUi("src/parts/editor/editor-panel.css"));

/** Every style the editor injected into the document, as one string. */
function injectedCss() {
  const parts = [];
  for (const style of window.document.querySelectorAll("style")) {
    parts.push(style.textContent ?? "");
    try {
      if (style.sheet) parts.push([...style.sheet.cssRules].map((rule) => rule.cssText).join("\n"));
    } catch {
      // A sheet jsdom cannot enumerate adds nothing beyond its text.
    }
  }
  return parts.join("\n");
}

/** Whether some rule whose selector satisfies `matches` declares `property` as `expected`, tokens resolved. */
function declares(matches, property, expected) {
  return rulesOf(injectedCss()).some(
    (rule) => rule.selectors.some(matches) && resolve(declaration(rule.body, property)) === expected,
  );
}
const endsWith = (suffix) => (selector) => selector.endsWith(suffix);

/** An SVG string as the DOM serializes it, so it compares with an element's innerHTML. */
function iconHtml(svg) {
  const holder = window.document.createElement("span");
  holder.innerHTML = svg;
  return holder.innerHTML;
}

/** The value a stylesheet rule list holds for a selector's property, tokens resolved. */
function panelValue(selector, property) {
  let found;
  for (const rule of panelRules) {
    if (!rule.selectors.includes(selector)) continue;
    const value = declaration(rule.body, property);
    if (value !== undefined) found = value;
  }
  return resolve(found);
}

{
  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: "C:\\project\\idioms.txt", text: "const a = 1;\nlet b = 2;\n" });
  const view = surface.editorView();

  // The has-selection class rides on the editor through editorAttributes, so
  // the stylesheet can hide the current-line wash whenever a selection exists.
  check("no has-selection class while nothing is selected", !view.dom.classList.contains("ws-has-selection"));
  view.dispatch({ selection: { anchor: 0, head: 5 } });
  check("a non-empty selection sets the has-selection class", view.dom.classList.contains("ws-has-selection"));
  view.dispatch({ selection: { anchor: 2 } });
  check("collapsing the selection clears the class", !view.dom.classList.contains("ws-has-selection"));

  check("the cursor blinks once a second", getDrawSelectionConfig(view.state).cursorBlinkRate === 1000);
  check("the indent unit is four spaces", view.state.facet(indentUnit) === "    " && getIndentUnit(view.state) === 4);
  check(
    "scroll past end pads the document so the last line can reach the top",
    Number.parseFloat(view.contentDOM.style.paddingBottom) > 0,
  );

  // Alt+click adds a cursor; Shift+Alt+drag selects a column; Alt alone shows no crosshair.
  const click = (init) => ({ altKey: false, shiftKey: false, ctrlKey: false, metaKey: false, ...init });
  const addsRange = (event) => view.state.facet(EditorView.clickAddsSelectionRange).some((fn) => fn(event));
  check("Alt+click adds a cursor", addsRange(click({ altKey: true })));
  check("a plain click does not add a cursor", !addsRange(click({})));
  const drag = (init) => ({ button: 0, clientX: 0, clientY: 0, ...click(init) });
  const rectangular = (event) =>
    view.state.facet(EditorView.mouseSelectionStyle).some((style) => style(view, event) !== null);
  check("Shift+Alt+drag selects a column", rectangular(drag({ altKey: true, shiftKey: true })));
  check("Alt+drag alone is not a column selection", !rectangular(drag({ altKey: true })));
  view.contentDOM.dispatchEvent(
    new window.KeyboardEvent("keydown", { key: "Alt", keyCode: 18, altKey: true, bubbles: true }),
  );
  check("pressing Alt draws no crosshair cursor", view.contentDOM.style.cursor !== "crosshair");

  // The gutter: a glyph lane, then line numbers, then the fold lane.
  const lanes = [...view.dom.querySelector(".cm-gutters").children].map((lane) => lane.className);
  const laneIndex = (name) => lanes.findIndex((className) => className.includes(name));
  check(
    "the gutter runs glyph margin, line numbers, fold lane",
    laneIndex("cm-glyphMargin") === 0 && laneIndex("cm-lineNumbers") === 1 && laneIndex("cm-foldGutter") === 2,
  );

  // A folded range reads as a bare ellipsis.
  view.dispatch({ effects: foldEffect.of({ from: 5, to: 10 }) });
  check(
    "a folded range shows a one-character ellipsis placeholder",
    view.contentDOM.querySelector(".cm-foldPlaceholder")?.textContent === "\u22EF",
  );
  // The fold lane marks the folded range with a right chevron that stays visible.
  // The lane's first element is CodeMirror's hidden width spacer; the line markers follow it.
  const foldedMarkers = [...view.dom.querySelectorAll('.cm-foldGutter .cm-gutterElement .ws-fold-marker[data-state="folded"]')].filter(
    (marker) => marker.parentElement.style.visibility !== "hidden",
  );
  check(
    "the folded range's lane shows one folded chevron-right marker",
    foldedMarkers.length === 1 && foldedMarkers[0].innerHTML === iconHtml(ICON_CHEVRON_RIGHT),
  );
  check(
    "the folded marker stays visible without gutter hover",
    declares((selector) => selector.endsWith(".ws-fold-marker[data-state=folded]"), "opacity", "1"),
  );

  // The look, as the theme declares it.
  check("text is 14px", declares(endsWith(".cm-scroller"), "font-size", "14px"));
  check("lines are 19px tall", declares(endsWith(".cm-scroller"), "line-height", "19px"));
  check("the gutter is #181818", declares(endsWith(".cm-gutters"), "background-color", "#181818"));
  check("the gutter has no right border", declares(endsWith(".cm-gutters"), "border-right", "none"));
  check("line numbers are #F0F0F05C", declares(endsWith(".cm-gutters"), "color", "#f0f0f05c"));
  check(
    "the active line number is #F0F0F0",
    declares(endsWith(".cm-activeLineGutter"), "color", "#f0f0f0"),
  );
  check(
    "the active line's gutter takes no tint",
    declares(endsWith(".cm-activeLineGutter"), "background-color", "transparent"),
  );
  check("line numbers are at least five characters wide", declares(endsWith(".cm-lineNumbers .cm-gutterElement"), "min-width", "5ch"));
  check("the glyph lane is 19px", declares(endsWith(".cm-glyphMargin"), "min-width", "19px"));
  check("the fold lane is 26px", declares(endsWith(".cm-foldGutter"), "min-width", "26px"));
  check("the content has no padding", declares(endsWith(".cm-content"), "padding", "0"));
  check("the current line is #262626", declares(endsWith(".cm-activeLine"), "background-color", "#262626"));
  check(
    "the current line wash is hidden while a selection exists",
    declares((selector) => selector.includes("ws-has-selection") && selector.endsWith(".cm-activeLine"), "background-color", "transparent"),
  );
  check(
    "a focused selection is #40404099 over the long selection-layer selector",
    declares(
      (selector) => selector.endsWith(".cm-selectionLayer .cm-selectionBackground") && selector.includes(".cm-focused"),
      "background",
      "#40404099",
    ),
  );
  check(
    "an unfocused selection is #40404077 over the long selection-layer selector",
    declares(
      (selector) => selector.endsWith(".cm-selectionLayer .cm-selectionBackground") && !selector.includes(".cm-focused"),
      "background",
      "#40404077",
    ),
  );
  check(
    "selection corners are 3px",
    declares((selector) => selector.endsWith(".cm-selectionBackground"), "border-radius", "3px"),
  );
  check("the cursor is a 2px bar", declares(endsWith(".cm-cursor"), "border-left-width", "2px"));
  check("the cursor is #F0F0F0", declares(endsWith(".cm-cursor"), "border-left-color", "#f0f0f0"));
  check("the cursor sits 1px left", declares(endsWith(".cm-cursor"), "margin-left", "-1px"));
  check("the editor draws no focus outline", declares(endsWith(".cm-focused"), "outline", "none"));
  check("find matches are #88C0D044", declares(endsWith(".cm-searchMatch"), "background-color", "#88c0d044"));
  check(
    "the current find match is #88C0D066",
    declares((selector) => selector.endsWith(".cm-searchMatch-selected"), "background-color", "#88c0d066"),
  );
  check("find matches have no outline", declares(endsWith(".cm-searchMatch"), "outline", "none"));
  check(
    "the word under the cursor is #F0F0F01E",
    declares(endsWith(".cm-selectionMatch-main"), "background-color", "#f0f0f01e"),
  );
  check(
    "other occurrences of a selection are #404040CC",
    declares(endsWith(".cm-selectionMatch"), "background-color", "#404040cc"),
  );
  check(
    "a bracket match is a #F0F0F01E box with a transparent 1px border",
    declares(endsWith(".cm-matchingBracket"), "background-color", "#f0f0f01e") &&
      declares(endsWith(".cm-matchingBracket"), "border", "1px solid transparent"),
  );
  check(
    "a bracket match shows with the editor unfocused",
    declares((selector) => selector.endsWith(".cm-matchingBracket") && !selector.includes(".cm-focused"), "background-color", "#f0f0f01e"),
  );
  check("fold chevrons are #C5C5C5", declares((selector) => selector.includes("cm-foldGutter"), "color", "#c5c5c5"));
  check(
    "fold chevrons fade in over 0.5s",
    declares((selector) => selector.includes("cm-foldGutter"), "transition", "opacity 0.5s"),
  );
  check("the folded placeholder is #808080", declares(endsWith(".cm-foldPlaceholder"), "color", "#808080"));
  check(
    "whitespace dots are drawn in #505050B3",
    rulesOf(injectedCss()).some(
      (rule) =>
        rule.selectors.some(endsWith(".cm-highlightSpace")) &&
        resolve(declaration(rule.body, "background-image"))?.includes("#505050b3"),
    ),
  );

  // Every syntax token reads Cursor Dark's tokenColors through an --editor-token-* value.
  const TOKENS = {
    keyword: "#82d2ce",
    operator: "#d6d6dd",
    punctuation: "#d6d6dd",
    string: "#e394dc",
    number: "#ebc88d",
    constant: "#f0f0f0",
    comment: "#f0f0f099",
    type: "#efb080",
    function: "#efb080",
    variable: "#87c3ff",
    "constant-variable": "#aaa0fa",
    property: "#aaa0fa",
    attribute: "#aaa0fa",
    tag: "#87c3ff",
    "tag-punctuation": "#a4a4a4",
    heading: "#88c0d0",
    link: "#82d2ce",
    strong: "#f8c762",
    emphasis: "#82d2ce",
    invalid: "#d6d6dd",
  };
  const css = injectedCss();
  for (const [name, value] of Object.entries(TOKENS)) {
    check(`--editor-token-${name} is ${value}`, resolve(`var(--editor-token-${name})`) === value);
    check(`the highlight style reads --editor-token-${name}`, css.includes(`var(--editor-token-${name})`));
  }
  check(
    "comments are italic",
    rulesOf(css).some(
      (rule) => declaration(rule.body, "font-style") === "italic" && declaration(rule.body, "color")?.includes("--editor-token-comment"),
    ),
  );

  surface.dispose();
}

// --- The fold lane's open chevron ----------------------------------------------------

{
  // A foldable line shows a down chevron while open; the JSON mode loads lazily.
  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: "C:\\project\\fold.json", text: '{\n  "a": 1\n}\n' });
  const view = surface.editorView();
  const openMarker = () => view.dom.querySelector('.cm-foldGutter .ws-fold-marker[data-state="open"]');
  for (let attempt = 0; attempt < 50 && openMarker() === null; attempt += 1) {
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
  const marker = openMarker();
  check("a foldable line shows an open fold marker", marker !== null);
  check(
    "the open fold marker is a chevron-down, not the folded chevron-right",
    marker?.innerHTML === iconHtml(ICON_CHEVRON_DOWN) && marker.innerHTML !== iconHtml(ICON_CHEVRON_RIGHT),
  );
  surface.dispose();
}

// --- The scrollbars and the find widget's stylesheet ---------------------------------

{
  const bar = ".ws-editor-surface .cm-scroller::-webkit-scrollbar";
  check("the vertical scrollbar is 14px", panelValue(bar, "width") === "14px");
  check("the horizontal scrollbar is 12px", panelValue(bar, "height") === "12px");
  check("scrollbar thumbs are square", panelValue(`${bar}-thumb`, "border-radius") === "0");
  check("the scrollbar thumb is #F0F0F011", panelValue(`${bar}-thumb`, "background-color") === "#f0f0f011");
  check("a hovered thumb is #F0F0F01E", panelValue(`${bar}-thumb:hover`, "background-color") === "#f0f0f01e");
  check("the find widget floats", panelValue(".ws-find-widget", "position") === "absolute");
  check("the find widget is 419px wide", panelValue(".ws-find-widget", "inline-size") === "419px");
  check("the find widget sits 28px from the right", panelValue(".ws-find-widget", "inset-inline-end") === "28px");
  check("find widget rows are 33px", panelValue(".ws-find-row", "block-size") === "33px");
}

// --- The find widget -------------------------------------------------------------------

{
  const surface = new CodeMirrorSurface();
  window.document.body.appendChild(surface.element);
  surface.open({ path: "C:\\project\\find.txt", text: "alpha beta\nthird line\n" });
  const view = surface.editorView();
  check("the find widget is not open until asked for", view.dom.querySelector(".ws-find-widget") === null);
  openSearchPanel(view);
  const widget = view.dom.querySelector(".ws-find-widget");
  check("find opens Cursor's widget, not the stock search panel", widget !== null && view.dom.querySelector(".cm-search") === null);
  check("the widget sits in a CodeMirror panel", widget?.closest(".cm-panel") !== null);

  const find = widget.querySelector("input[name=search]");
  const replace = widget.querySelector("input[name=replace]");
  check("the find field says Find", find?.getAttribute("placeholder") === "Find" && find.getAttribute("aria-label") === "Find");
  check(
    "the replace field says Replace",
    replace?.getAttribute("placeholder") === "Replace" && replace.getAttribute("aria-label") === "Replace",
  );
  const labels = [...widget.querySelectorAll("button")].map((button) => button.getAttribute("aria-label")).sort();
  const EXPECTED = [
    "Close (Escape)",
    "Match Case (Alt+C)",
    "Match Whole Word (Alt+W)",
    "Next Match (Enter)",
    "Previous Match (Shift+Enter)",
    "Replace (Enter)",
    "Replace All (Ctrl+Alt+Enter)",
    "Toggle Replace",
    "Use Regular Expression (Alt+R)",
  ].sort();
  check(`the buttons carry Cursor's labels (${labels.join(" | ")})`, labels.join("|") === EXPECTED.join("|"));
  check(
    "every button's tooltip is its label",
    [...widget.querySelectorAll("button")].every((button) => button.title === button.getAttribute("aria-label")),
  );
  const button = (label) => widget.querySelector(`button[aria-label="${label}"]`);
  const count = () => widget.querySelector(".ws-find-count")?.textContent;
  const type = (input, value) => {
    input.value = value;
    input.dispatchEvent(new window.Event("input", { bubbles: true }));
  };
  const press = (input, init) =>
    input.dispatchEvent(new window.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));

  check("the replace row starts hidden", widget.dataset.replace === "false");
  button("Toggle Replace").click();
  check("Toggle Replace shows the replace row", widget.dataset.replace === "true");
  button("Toggle Replace").click();
  check("Toggle Replace again hides it", widget.dataset.replace === "false");
  revealReplace(view);
  check("revealReplace shows the replace row for Replace (Ctrl+H)", widget.dataset.replace === "true");

  check("an empty query reads No results", count() === "No results");
  type(find, "zzz");
  check("a query with no match reads No results", count() === "No results");
  type(find, "a");
  check("typing sets the search query", getSearchQuery(view.state).search === "a");
  check("matches are counted, the current one unknown", count() === "? of 3");
  press(find, { key: "Enter", keyCode: 13 });
  check(
    "Enter goes to the first match and counts it",
    view.state.selection.main.from === 0 && view.state.selection.main.to === 1 && count() === "1 of 3",
  );
  press(find, { key: "Enter", keyCode: 13 });
  check("Enter again goes to the next match", view.state.selection.main.from === 4 && count() === "2 of 3");
  press(find, { key: "Enter", keyCode: 13, shiftKey: true });
  check("Shift+Enter goes back", view.state.selection.main.from === 0 && count() === "1 of 3");
  button("Next Match (Enter)").click();
  button("Previous Match (Shift+Enter)").click();
  check("the Next and Previous buttons walk the matches", view.state.selection.main.from === 0 && count() === "1 of 3");

  // The count rescans the document only when its answer can change: an update
  // that moves nothing (scroll, focus, geometry) must not walk every match.
  const queryProto = Object.getPrototypeOf(getSearchQuery(view.state));
  const realGetCursor = queryProto.getCursor;
  let scans = 0;
  queryProto.getCursor = function countedGetCursor(...args) {
    scans += 1;
    return realGetCursor.apply(this, args);
  };
  try {
    view.dispatch({});
    view.dispatch({ effects: EditorView.scrollIntoView(0) });
    view.requestMeasure();
    check("updates that change neither query, text, nor selection do not rescan", scans === 0);
  } finally {
    queryProto.getCursor = realGetCursor;
  }
  view.dispatch({ selection: { anchor: 4, head: 5 } });
  check("a moved selection recounts", count() === "2 of 3");
  view.dispatch({ changes: { from: view.state.doc.length, insert: "a" } });
  check("an edit recounts", count() === "2 of 4");
  view.dispatch({ changes: { from: view.state.doc.length - 1, to: view.state.doc.length } });
  view.dispatch({ selection: { anchor: 0, head: 1 } });
  check("undoing the edit and moving back recounts", count() === "1 of 3");

  const toggles = ["Match Case (Alt+C)", "Match Whole Word (Alt+W)", "Use Regular Expression (Alt+R)"];
  check("the option toggles start unpressed", toggles.every((label) => button(label).getAttribute("aria-pressed") === "false"));
  button("Match Case (Alt+C)").click();
  check(
    "Match Case sets the query's flag and shows pressed",
    getSearchQuery(view.state).caseSensitive === true && button("Match Case (Alt+C)").getAttribute("aria-pressed") === "true",
  );
  press(find, { key: "w", altKey: true });
  check("Alt+W toggles whole word from the find field", getSearchQuery(view.state).wholeWord === true);
  press(find, { key: "r", altKey: true });
  check("Alt+R toggles the regular expression", getSearchQuery(view.state).regexp === true);
  press(find, { key: "c", altKey: true });
  check("Alt+C toggles match case back off", getSearchQuery(view.state).caseSensitive === false);
  button("Match Whole Word (Alt+W)").click();
  button("Use Regular Expression (Alt+R)").click();

  // Replace through the widget.
  type(find, "alpha");
  type(replace, "ALPHA");
  press(find, { key: "Enter", keyCode: 13 });
  button("Replace (Enter)").click();
  check("Replace swaps the current match", surface.text() === "ALPHA beta\nthird line\n");
  type(find, "i");
  type(replace, "!");
  press(replace, { key: "Enter", keyCode: 13, ctrlKey: true, altKey: true });
  check("Ctrl+Alt+Enter replaces every match", surface.text() === "ALPHA beta\nth!rd l!ne\n");
  type(find, "e");
  button("Replace All (Ctrl+Alt+Enter)").click();
  check("Replace All swaps every match", surface.text() === "ALPHA b!ta\nth!rd l!n!\n");
  type(find, "th!rd");
  type(replace, "third");
  press(find, { key: "Enter", keyCode: 13 });
  press(replace, { key: "Enter", keyCode: 13 });
  check("Enter in the replace field replaces the current match", surface.text() === "ALPHA b!ta\nthird l!n!\n");

  press(find, { key: "Escape", keyCode: 27 });
  check("Escape closes the widget", !searchPanelOpen(view.state) && view.dom.querySelector(".ws-find-widget") === null);
  openSearchPanel(view);
  view.dom.querySelector('button[aria-label="Close (Escape)"]').click();
  check("the Close button closes it", !searchPanelOpen(view.state));
  openSearchPanel(view);
  closeSearchPanel(view);
  surface.dispose();
}

if (failures.length > 0) {
  console.error(`editor-idioms: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("editor-idioms: all assertions passed");
process.exit(0);
