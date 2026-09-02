// The mention typeahead popup (src/ui/workshop/typeahead-popup.ts) in
// jsdom, driven through a real editor over the MentionChip wiring from
// mention-chip.ts. Covers: the stub source filters its three canned
// entries by query; typing "@" opens the popup with listbox semantics,
// a highlighted first row, and inline position styles written by the
// managed mount (jsdom layout is zero, so the positioning contract is
// pinned by the styles being written from the virtual-element rect, not
// by pixel values); a no-match query hides the popup, and Enter while
// it is hidden inserts nothing; mousedown on the popup is
// default-prevented so the editor keeps focus; ArrowUp/ArrowDown move
// the highlight with wraparound, and narrowing the query clamps the
// highlight to the first matching row; Enter inserts the highlighted
// mention node and closes the popup; clicking a row does the same;
// Escape dismisses the session and it stays dismissed while typing;
// destroying the editor mid-session removes the popup; inside
// PromptInput, Enter with the popup open selects instead of submitting.
// Runs under the shared leak check: a popup or PromptInput that is
// never disposed fails.
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
      export { PromptInput } from "./src/ui/prompt-input.ts";
      export { MentionChip } from "./src/ui/workshop/mention-chip.ts";
      export { mentionTypeaheadItems } from "./src/ui/workshop/typeahead-popup.ts";
      export { Editor } from "@tiptap/core";
      export { StarterKit } from "@tiptap/starter-kit";
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

const bundlePath = path.join(os.tmpdir(), "promptforge-typeahead-popup-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, PromptInput, MentionChip, mentionTypeaheadItems, Editor, StarterKit } =
  await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The suggestion session only activates for a focused, connected editor:
// ProseMirror syncs the DOM selection (which the mention command's
// collapseToEnd needs) only when the view has focus.
function createEditor() {
  const element = document.createElement("div");
  document.body.appendChild(element);
  const editor = new Editor({
    element,
    extensions: [StarterKit, MentionChip],
  });
  editor.commands.focus();
  return { editor, element };
}

function flush() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

// insertContent dispatches the same transaction typing would; the flush
// lets the suggestion plugin's async item fetch and the mount's
// computePosition settle.
async function typeText(editor, text) {
  editor.commands.insertContent(text);
  await flush();
}

function pressKey(target, key) {
  target.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
  );
}

function popup() {
  return document.body.querySelector(".typeahead-popup");
}

function popupItems() {
  return [...(popup()?.querySelectorAll(".typeahead-popup__item") ?? [])];
}

function selectedItem() {
  return popup()?.querySelector(".typeahead-popup__item--selected") ?? null;
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

await assertNoLeaks(lifecycle, async () => {
  // --- Stub source --------------------------------------------------------

  {
    const items = mentionTypeaheadItems("");
    check(
      "the stub source returns three canned entries",
      items.length === 3 &&
        items[0].label === "README.md" &&
        items[1].label === "src/main.ts" &&
        items[2].label === "Cargo.toml",
    );
    check(
      "the stub source filters by case-insensitive substring",
      mentionTypeaheadItems("RE").length === 1 &&
        mentionTypeaheadItems("re").length === 1 &&
        mentionTypeaheadItems("zzz").length === 0,
    );
  }

  // --- Open -----------------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@");
    const el = popup();
    check("typing @ opens the popup", el !== null && el.isConnected);
    const items = popupItems();
    check(
      "the popup lists the stub entries",
      items.length === 3 &&
        items[0].textContent === "README.md" &&
        items[1].textContent === "src/main.ts" &&
        items[2].textContent === "Cargo.toml",
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
        items[0].classList.contains("typeahead-popup__item--selected") &&
        items[0].getAttribute("aria-selected") === "true" &&
        items[1]?.getAttribute("aria-selected") === "false",
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
    editor.destroy();
    element.remove();
  }

  // --- Filter -----------------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@RE");
    const items = popupItems();
    check(
      "typing a query filters the popup entries",
      items.length === 1 && items[0]?.textContent === "README.md",
    );
    await typeText(editor, "zz");
    check(
      "a query with no matches hides the popup",
      popup()?.hidden === true && popupItems().length === 0,
    );
    pressKey(editor.view.dom, "Enter");
    check(
      "Enter with no matching items inserts nothing",
      !mentionInDoc(editor) && editor.getText() === "@REzz",
    );
    editor.destroy();
    element.remove();
  }

  // --- Keyboard navigation ------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@");
    const items = popupItems();
    pressKey(editor.view.dom, "ArrowDown");
    check(
      "ArrowDown moves the highlight to the next item",
      items[1] !== undefined && selectedItem() === items[1],
    );
    pressKey(editor.view.dom, "ArrowDown");
    pressKey(editor.view.dom, "ArrowDown");
    check(
      "ArrowDown wraps from the last item to the first",
      items[0] !== undefined && selectedItem() === items[0],
    );
    pressKey(editor.view.dom, "ArrowUp");
    check(
      "ArrowUp wraps from the first item to the last",
      items[2] !== undefined && selectedItem() === items[2],
    );
    check(
      "the highlighted row carries aria-selected",
      selectedItem()?.getAttribute("aria-selected") === "true",
    );
    await typeText(editor, "RE");
    check(
      "narrowing the query clamps the highlight to the first matching row",
      popupItems().length === 1 && selectedItem() === popupItems()[0],
    );
    editor.destroy();
    element.remove();
  }

  // --- Enter selects --------------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@");
    pressKey(editor.view.dom, "ArrowDown");
    pressKey(editor.view.dom, "Enter");
    check("Enter inserts the highlighted mention", mentionInDoc(editor));
    const attrs = mentionAttrs(editor);
    check(
      "the inserted mention carries the highlighted item",
      attrs?.id === "src/main.ts" && attrs?.label === "src/main.ts",
    );
    check("selecting closes the popup", popup() === null);
    // getText renders the mention through its renderText ("@label"), so
    // the query range being replaced reads as the mention plus the
    // trailing space the command inserts.
    check(
      "the mention replaces the query text",
      editor.getText() === "@src/main.ts ",
    );
    editor.destroy();
    element.remove();
  }

  // --- Escape dismisses -------------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@RE");
    pressKey(editor.view.dom, "Escape");
    check("Escape closes the popup", popup() === null);
    check("Escape leaves the typed query in place", editor.getText() === "@RE");
    check("Escape inserts no mention", !mentionInDoc(editor));
    await typeText(editor, "A");
    check("a dismissed session stays dismissed while typing", popup() === null);
    editor.destroy();
    element.remove();
  }

  // --- Click selects ------------------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@");
    const items = popupItems();
    items[2]?.click();
    check("clicking a row inserts its mention", mentionInDoc(editor));
    check(
      "the clicked mention carries the row's item",
      mentionAttrs(editor)?.id === "Cargo.toml",
    );
    check("clicking closes the popup", popup() === null);
    editor.destroy();
    element.remove();
  }

  // --- Destroy mid-session ---------------------------------------------------------------

  {
    const { editor, element } = createEditor();
    await typeText(editor, "@");
    editor.destroy();
    check("destroying the editor mid-session removes the popup", popup() === null);
    element.remove();
  }

  // --- PromptInput integration --------------------------------------------------------------

  {
    let submitted = 0;
    const input = new PromptInput({
      onSubmit: () => {
        submitted++;
      },
    });
    document.body.appendChild(input.element);
    const editorDom = input.element.querySelector(".prompt-input__editor");
    // Tiptap stamps the Editor instance on the view DOM (dom.editor);
    // the test drives commands through it because PromptInput does not
    // expose its editor.
    const editor = editorDom.editor;
    editor.commands.focus();
    await typeText(editor, "@");
    pressKey(editorDom, "Enter");
    check(
      "Enter with the typeahead open selects instead of submitting",
      submitted === 0 && input.element.querySelector(".mention-chip") !== null,
    );
    check("the selection closed the popup", popup() === null);
    pressKey(editorDom, "Enter");
    check("Enter with no typeahead open submits", submitted === 1);
    input.dispose();
    input.element.remove();
  }
});

if (failures.length > 0) {
  console.error(`typeahead-popup: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("typeahead-popup: all assertions passed");
process.exit(0);
