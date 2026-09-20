// The mention typeahead popup (src/parts/chatbox/typeahead-popup.ts) in
// jsdom, driven through a ChatBox (src/parts/chatbox/chat-box.ts) so the
// suggestion plugin runs with the component's own configuration: the
// injected or stub mentionSource, the debounce, minQueryLength 0, and
// the Enter/Tab yield in the editor's key handler. Covers: typing "@"
// opens the popup with listbox semantics, a highlighted first row, and
// inline position styles written by the managed mount (jsdom layout is
// zero, so the positioning contract is pinned by the styles being
// written from the virtual-element rect, not by pixel values); a
// no-match query hides the popup, and Enter while it is hidden inserts
// nothing; mousedown on the popup is default-prevented so the editor
// keeps focus; ArrowUp/ArrowDown move the highlight with wraparound, and
// narrowing the query clamps the highlight to the first matching row;
// Enter inserts the highlighted mention node and closes the popup;
// clicking a row does the same; Escape dismisses the session and it
// stays dismissed while typing; destroying the editor mid-session
// removes the popup; inside ChatBox, Enter with the popup open selects
// instead of submitting. The extended rows: a chip's `description`
// renders dimmed beside the label; items with `group` render a
// non-selectable header at each group boundary and arrow navigation
// skips the headers; a `loading` row shows while the source is pending
// and gives way to the results; Tab accepts like Enter; a space closes
// the popup leaving the typed text; Backspace directly after a pill
// restores the literal "@" and reopens the popup. Runs under the shared
// leak check: a popup or ChatBox that is never disposed fails.
// Run: node test/typeahead-popup.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { ChatBox } from "./src/parts/chatbox/chat-box.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

// ProseMirror reads the DOM globals at construction, so the jsdom
// globals must exist before the bundle is imported. pretendToBeVisual
// supplies the requestAnimationFrame ProseMirror schedules with. The
// suggestion plugin's managed mount also touches the HTMLElement,
// Node, and DOMRect globals.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.Element = dom.window.Element;
globalThis.Node = dom.window.Node;
globalThis.DOMRect = dom.window.DOMRect;
// Tiptap's focus command reads requestAnimationFrame from the global
// scope, not from the view's window.
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window);
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window);
// jsdom's Range has no layout rects; ProseMirror's scroll-to-selection
// reads them when selecting a mention focuses the editor.
dom.window.Range.prototype.getClientRects = () => [];
dom.window.Range.prototype.getBoundingClientRect = () => new dom.window.DOMRect();

const bundlePath = path.join(os.tmpdir(), "promptforge-typeahead-popup-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, ChatBox } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The suggestion session only activates for a focused, connected editor:
// ProseMirror syncs the DOM selection (which the mention command's
// collapseToEnd needs) only when the view has focus. Tiptap stamps the
// Editor on the view DOM (dom.editor); the test drives commands through
// it because ChatBox does not expose its editor.
function createBox(props = {}, sink = () => {}) {
  const input = new ChatBox(props, sink);
  document.body.appendChild(input.element);
  const editorDom = input.element.querySelector(".ws-prompt-input__editor");
  const editor = editorDom.editor;
  editor.commands.focus();
  return {
    input,
    editor,
    editorDom,
    dispose() {
      input.dispose();
      input.element.remove();
    },
  };
}

// The suggestion plugin debounces its item fetch (the component
// configures 50 to 100 ms); a wait past that window lets the fetch and
// the mount's computePosition settle.
function settle() {
  return new Promise((resolve) => setTimeout(resolve, 160));
}

// insertContent dispatches the same transaction typing would.
async function typeText(editor, text) {
  editor.commands.insertContent(text);
  await settle();
}

function pressKey(target, key) {
  target.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
  );
}

function popup() {
  return document.body.querySelector(".ws-typeahead-popup");
}

function popupItems() {
  return [...(popup()?.querySelectorAll(".ws-typeahead-popup__item") ?? [])];
}

function popupLabels() {
  return popupItems().map((item) => item.querySelector(".ws-typeahead-popup__label")?.textContent);
}

function popupRows() {
  return [...(popup()?.querySelectorAll(".ws-typeahead-popup__list > li") ?? [])];
}

function selectedItem() {
  return popup()?.querySelector(".ws-typeahead-popup__item--selected") ?? null;
}

function mentionInDoc(editor) {
  let found = false;
  editor.state.doc.descendants((node) => {
    if (node.type.name === "mentionNode") found = true;
    return !found;
  });
  return found;
}

function mentionAttrs(editor) {
  let attrs;
  editor.state.doc.descendants((node) => {
    if (node.type.name === "mentionNode") attrs = node.attrs;
    return attrs === undefined;
  });
  return attrs;
}

// A source whose call resolves only when the test says so.
function deferredSource() {
  const calls = [];
  const source = (query, signal) =>
    new Promise((resolve) => {
      calls.push({ query, signal, resolve });
    });
  source.calls = calls;
  return source;
}

await assertNoLeaks(lifecycle, async () => {
  // --- Open -----------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    const el = popup();
    check("typing @ opens the popup", el !== null && el.isConnected);
    const items = popupItems();
    check(
      "the popup lists the stub entries",
      popupLabels().join(",") === "README.md,src/main.ts,Cargo.toml",
    );
    const list = el?.querySelector('ul[role="listbox"]');
    check(
      "the popup carries listbox semantics",
      list !== null &&
        list !== undefined &&
        items.every((item) => item.getAttribute("role") === "option"),
    );
    check(
      "the first item opens highlighted",
      items[0] !== undefined &&
        items[0].classList.contains("ws-typeahead-popup__item--selected") &&
        items[0].getAttribute("aria-selected") === "true" &&
        items[1]?.getAttribute("aria-selected") === "false",
    );
    check(
      "each row draws an icon slot beside its label",
      items.every((item) => item.querySelector(".ws-typeahead-popup__icon svg") !== null),
    );
    check(
      "the managed mount writes the popup position from the cursor rect",
      el !== null &&
        el.style.position === "absolute" &&
        el.style.left !== "" &&
        el.style.top !== "",
    );
    const mousedown = new dom.window.MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    el?.dispatchEvent(mousedown);
    check(
      "mousedown on the popup is default-prevented so the editor keeps focus",
      mousedown.defaultPrevented === true,
    );
    box.dispose();
  }

  // --- Filter -----------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@RE");
    check(
      "typing a query filters the popup entries",
      popupLabels().join(",") === "README.md",
    );
    await typeText(box.editor, "zz");
    check(
      "a query with no matches hides the popup",
      popup()?.hidden === true && popupItems().length === 0,
    );
    pressKey(box.editorDom, "Enter");
    check(
      "Enter with no matching items inserts nothing",
      !mentionInDoc(box.editor) && box.input.getText() === "@REzz",
    );
    box.dispose();
  }

  // --- Keyboard navigation ------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    const items = popupItems();
    pressKey(box.editorDom, "ArrowDown");
    check(
      "ArrowDown moves the highlight to the next item",
      items[1] !== undefined && selectedItem() === items[1],
    );
    pressKey(box.editorDom, "ArrowDown");
    pressKey(box.editorDom, "ArrowDown");
    check(
      "ArrowDown wraps from the last item to the first",
      items[0] !== undefined && selectedItem() === items[0],
    );
    pressKey(box.editorDom, "ArrowUp");
    check(
      "ArrowUp wraps from the first item to the last",
      items[2] !== undefined && selectedItem() === items[2],
    );
    check(
      "the highlighted row carries aria-selected",
      selectedItem()?.getAttribute("aria-selected") === "true",
    );
    await typeText(box.editor, "RE");
    check(
      "narrowing the query clamps the highlight to the first matching row",
      popupItems().length === 1 && selectedItem() === popupItems()[0],
    );
    box.dispose();
  }

  // --- Enter selects --------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    pressKey(box.editorDom, "ArrowDown");
    pressKey(box.editorDom, "Enter");
    check("Enter inserts the highlighted mention", mentionInDoc(box.editor));
    const attrs = mentionAttrs(box.editor);
    check(
      "the inserted mention carries the highlighted item",
      attrs?.id === "src/main.ts" && attrs?.label === "src/main.ts" && attrs?.kind === "file",
    );
    check("selecting closes the popup", popup() === null);
    // getText renders the mention through its renderText ("@label"), so
    // the query range being replaced reads as the mention plus the
    // trailing space the command inserts.
    check(
      "the mention replaces the query text",
      box.editor.getText() === "@src/main.ts ",
    );
    box.dispose();
  }

  // --- Tab selects ------------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    pressKey(box.editorDom, "ArrowDown");
    pressKey(box.editorDom, "ArrowDown");
    pressKey(box.editorDom, "Tab");
    check("Tab inserts the highlighted mention", mentionInDoc(box.editor));
    check(
      "the Tab-inserted mention carries the highlighted item",
      mentionAttrs(box.editor)?.id === "Cargo.toml",
    );
    check("Tab closes the popup", popup() === null);
    check(
      "the Tab insertion is followed by one space",
      box.editor.getText() === "@Cargo.toml ",
    );
    box.dispose();
  }

  // --- Escape dismisses -------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@RE");
    pressKey(box.editorDom, "Escape");
    check("Escape closes the popup", popup() === null);
    check("Escape leaves the typed query in place", box.editor.getText() === "@RE");
    check("Escape inserts no mention", !mentionInDoc(box.editor));
    await typeText(box.editor, "A");
    check("a dismissed session stays dismissed while typing", popup() === null);
    box.dispose();
  }

  // --- Space closes ---------------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@RE");
    check("the popup is open before the space", popup() !== null);
    await typeText(box.editor, " ");
    check("a space closes the popup", popup() === null);
    check(
      "the space leaves the typed text in place with no mention",
      box.editor.getText() === "@RE " && !mentionInDoc(box.editor),
    );
    box.dispose();
  }

  // --- Click selects ------------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    const items = popupItems();
    items[2]?.click();
    check("clicking a row inserts its mention", mentionInDoc(box.editor));
    check(
      "the clicked mention carries the row's item",
      mentionAttrs(box.editor)?.id === "Cargo.toml",
    );
    check("clicking closes the popup", popup() === null);
    box.dispose();
  }

  // --- Backspace after a pill -----------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    pressKey(box.editorDom, "Enter");
    check("the pill is in place before Backspace", mentionInDoc(box.editor));
    // The command leaves the cursor after the trailing space; jsdom
    // performs no native deletion, so place the cursor directly after
    // the pill (paragraph opens at 0, the atom spans 1..2) as a real
    // Backspace over the space would.
    box.editor.commands.setTextSelection(2);
    pressKey(box.editorDom, "Backspace");
    await settle();
    check(
      "Backspace directly after a pill restores the literal @",
      !mentionInDoc(box.editor) && box.editor.getText() === "@ ",
    );
    check(
      "the restored @ reopens the popup with the full list",
      popup() !== null && popupLabels().join(",") === "README.md,src/main.ts,Cargo.toml",
    );
    box.dispose();
  }

  // --- Destroy mid-session ---------------------------------------------------------------

  {
    const box = createBox();
    await typeText(box.editor, "@");
    box.input.dispose();
    check("disposing the box mid-session removes the popup", popup() === null);
    box.input.element.remove();
  }

  // --- Description column -------------------------------------------------------------------

  {
    const mentionSource = async () => [
      { id: "src/a.ts", label: "a.ts", kind: "file", description: "src", data: null },
      { id: "b.ts", label: "b.ts", kind: "file", data: null },
    ];
    const box = createBox({ mentionSource });
    await typeText(box.editor, "@");
    const items = popupItems();
    const description = items[0]?.querySelector(".ws-typeahead-popup__description");
    check(
      "a chip's description renders in its own dimmed slot after the label",
      description !== null &&
        description !== undefined &&
        description.textContent === "src" &&
        description.previousElementSibling?.classList.contains("ws-typeahead-popup__label") === true,
    );
    check(
      "a chip without a description renders no description slot",
      items[1]?.querySelector(".ws-typeahead-popup__description") === null,
    );
    check(
      "the description is not stored on the inserted node",
      (() => {
        pressKey(box.editorDom, "Enter");
        const attrs = mentionAttrs(box.editor);
        return attrs !== undefined && attrs.id === "src/a.ts" && !("description" in attrs);
      })(),
    );
    box.dispose();
  }

  // --- Group headers ----------------------------------------------------------------------------

  {
    const mentionSource = async () => [
      { id: "f1", label: "one.ts", group: "Files", data: null },
      { id: "d1", label: "src", kind: "folder", group: "Folders", data: null },
      { id: "f2", label: "two.ts", group: "Files", data: null },
      { id: "u", label: "ungrouped", data: null },
    ];
    const box = createBox({ mentionSource });
    await typeText(box.editor, "@");
    const rows = popupRows();
    const kinds = rows.map((row) =>
      row.classList.contains("ws-typeahead-popup__header")
        ? `H:${row.textContent}`
        : row.querySelector(".ws-typeahead-popup__label")?.textContent,
    );
    check(
      "items sort by group with a header at each boundary and ungrouped items first",
      kinds.join("|") === "ungrouped|H:Files|one.ts|two.ts|H:Folders|src",
    );
    check(
      "headers are not options",
      rows
        .filter((row) => row.classList.contains("ws-typeahead-popup__header"))
        .every((row) => row.getAttribute("role") === "presentation" && !row.hasAttribute("aria-selected")),
    );
    const items = popupItems();
    check("the first item opens highlighted, not a header", selectedItem() === items[0]);
    pressKey(box.editorDom, "ArrowDown");
    check("ArrowDown skips the Files header", selectedItem() === items[1]);
    pressKey(box.editorDom, "ArrowDown");
    pressKey(box.editorDom, "ArrowDown");
    check("ArrowDown skips the Folders header", selectedItem() === items[3]);
    pressKey(box.editorDom, "ArrowDown");
    check("ArrowDown wraps over items only", selectedItem() === items[0]);
    pressKey(box.editorDom, "ArrowUp");
    check("ArrowUp wraps to the last item, not a header", selectedItem() === items[3]);
    pressKey(box.editorDom, "Enter");
    check(
      "Enter inserts the highlighted grouped item",
      mentionAttrs(box.editor)?.id === "d1",
    );
    box.dispose();
  }

  // --- Loading state -----------------------------------------------------------------------------

  {
    const mentionSource = deferredSource();
    const box = createBox({ mentionSource });
    await typeText(box.editor, "@");
    const loading = popup()?.querySelector(".ws-typeahead-popup__loading");
    check(
      "a loading row shows while the source is pending and the popup stays visible",
      mentionSource.calls.length === 1 &&
        loading !== null &&
        loading !== undefined &&
        popup()?.hidden === false &&
        popupItems().length === 0,
    );
    pressKey(box.editorDom, "Enter");
    check(
      "Enter while loading inserts nothing",
      !mentionInDoc(box.editor) && box.editor.getText() === "@",
    );
    mentionSource.calls[0].resolve([{ id: "r", label: "ready.ts", data: null }]);
    await settle();
    check(
      "the results replace the loading row",
      popup()?.querySelector(".ws-typeahead-popup__loading") === null &&
        popupLabels().join(",") === "ready.ts",
    );
    box.dispose();
  }

  // --- ChatBox integration --------------------------------------------------------------

  {
    let submitted = 0;
    const box = createBox({}, (event) => {
      if (event.type === "send") {
        submitted++;
      }
    });
    await typeText(box.editor, "@");
    pressKey(box.editorDom, "Enter");
    check(
      "Enter with the typeahead open selects instead of submitting",
      submitted === 0 && box.input.element.querySelector(".ws-mention-chip") !== null,
    );
    check("the selection closed the popup", popup() === null);
    pressKey(box.editorDom, "Enter");
    check("Enter with no typeahead open submits", submitted === 1);
    box.dispose();
  }
});

if (failures.length > 0) {
  console.error(`ws-typeahead-popup: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ws-typeahead-popup: all assertions passed");
process.exit(0);
