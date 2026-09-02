// The prompt input (src/ui/prompt-input.ts) in jsdom: a Tiptap/
// ProseMirror editor framed as the chat box. Covers: the editor mounts
// inside the framed container with an accessible editable region; the
// placeholder decorates the empty paragraph and lifts once content
// lands; Enter submits through onSubmit while an IME-composition Enter
// and Shift+Enter do not (Shift+Enter inserts a hard break); the box
// height tracks content clamped between the min/max tokens (jsdom
// reports scrollHeight 0, so the test stubs it to drive the clamp, and
// pins the exported clamp directly); getText returns paragraphs and
// breaks as single newlines; clear empties; setEditable toggles
// contenteditable; dispose destroys the editor. Runs under the shared
// leak check: a PromptInput that is never disposed fails.
// Run: node test/prompt-input.mjs
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
      export { PromptInput, clampPromptInputHeight } from "./src/ui/prompt-input.ts";
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
  // The module under test imports its colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

// ProseMirror reads the DOM globals at construction, so the jsdom
// globals must exist before the bundle is imported. pretendToBeVisual
// supplies the requestAnimationFrame ProseMirror schedules with.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);

const bundlePath = path.join(os.tmpdir(), "promptforge-prompt-input-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, PromptInput, clampPromptInputHeight } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function pressEnter(target, init = {}) {
  target.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
      ...init,
    }),
  );
}

function editorElement(input) {
  return input.element.querySelector(".prompt-input__editor");
}

await assertNoLeaks(lifecycle, () => {
  // --- Mount ----------------------------------------------------------------

  {
    const input = new PromptInput();
    const editor = editorElement(input);
    check(
      "the editor mounts a ProseMirror region inside the framed container",
      input.element.classList.contains("prompt-input") &&
        editor !== null &&
        editor.classList.contains("ProseMirror"),
    );
    check(
      "the editable region is contenteditable with an accessible name",
      editor.getAttribute("contenteditable") === "true" &&
        editor.getAttribute("role") === "textbox" &&
        editor.getAttribute("aria-label") === "Message" &&
        editor.getAttribute("aria-multiline") === "true",
    );
    input.dispose();
  }

  // --- Placeholder ------------------------------------------------------------

  {
    const input = new PromptInput({ placeholder: "Message the agent" });
    const empty = editorElement(input).querySelector("p");
    check(
      "the empty paragraph carries the placeholder decoration",
      empty !== null &&
        empty.classList.contains("is-editor-empty") &&
        empty.getAttribute("data-placeholder") === "Message the agent",
    );
    const filled = new PromptInput({ content: "<p>hello</p>" });
    const paragraph = editorElement(filled).querySelector("p");
    check(
      "content lifts the placeholder decoration",
      paragraph !== null && !paragraph.classList.contains("is-editor-empty"),
    );
    input.dispose();
    filled.dispose();
  }

  // --- Submit -----------------------------------------------------------------

  {
    let submitted = 0;
    const input = new PromptInput({
      content: "<p>hello</p>",
      onSubmit: () => {
        submitted++;
      },
    });
    const editor = editorElement(input);
    pressEnter(editor);
    check("Enter submits", submitted === 1);
    check(
      "a submitting Enter leaves the text untouched",
      input.getText() === "hello",
    );
    input.dispose();
  }

  {
    let submitted = 0;
    const input = new PromptInput({
      content: "<p>hello</p>",
      onSubmit: () => {
        submitted++;
      },
    });
    const editor = editorElement(input);
    // A full composition session: ProseMirror tracks composing state
    // from compositionstart, so the committing Enter is inert end to end.
    editor.dispatchEvent(new dom.window.CompositionEvent("compositionstart", { bubbles: true }));
    pressEnter(editor, { isComposing: true });
    check(
      "an Enter committing an IME composition does not submit",
      submitted === 0,
    );
    check(
      "an Enter committing an IME composition leaves the text untouched",
      input.getText() === "hello",
    );
    editor.dispatchEvent(new dom.window.CompositionEvent("compositionend", { bubbles: true }));
    // A bare isComposing flag, with no session ProseMirror tracked: the
    // guard in the keydown handler is the only thing refusing the send.
    pressEnter(editor, { isComposing: true });
    check(
      "an Enter flagged isComposing without a tracked session still does not submit",
      submitted === 0,
    );
    input.dispose();
  }

  {
    let submitted = 0;
    const input = new PromptInput({
      content: "<p>hello</p>",
      onSubmit: () => {
        submitted++;
      },
    });
    const editor = editorElement(input);
    pressEnter(editor, { shiftKey: true });
    check("Shift+Enter does not submit", submitted === 0);
    check(
      "Shift+Enter inserts a hard break",
      editor.querySelector("br:not(.ProseMirror-trailingBreak)") !== null &&
        input.getText() === "\nhello",
    );
    input.dispose();
  }

  // --- Auto-resize --------------------------------------------------------------

  check(
    "the clamp passes heights inside the band through",
    clampPromptInputHeight(150, 36, 200) === 150,
  );
  check(
    "the clamp holds heights at the max token",
    clampPromptInputHeight(500, 36, 200) === 200,
  );
  check(
    "the clamp lifts heights to the min token",
    clampPromptInputHeight(10, 36, 200) === 36,
  );

  {
    const input = new PromptInput({ content: "<p>hello</p>" });
    const editor = editorElement(input);
    let measured = 150;
    // jsdom reports scrollHeight 0; the stub stands in for layout.
    Object.defineProperty(editor, "scrollHeight", {
      configurable: true,
      get: () => measured,
    });
    input.syncHeight();
    check(
      "the box height follows the content inside the band",
      editor.style.height === "150px",
    );
    measured = 500;
    input.syncHeight();
    check(
      "the box height clamps at the max token",
      editor.style.height === "200px",
    );
    measured = 10;
    input.syncHeight();
    check(
      "the box height clamps at the min token",
      editor.style.height === "36px",
    );
    measured = 120;
    input.clear();
    check(
      "an edit re-measures the box",
      editor.style.height === "120px",
    );
    input.dispose();
  }

  // --- Text extraction -----------------------------------------------------------

  {
    const input = new PromptInput({ content: "<p>first</p><p>second</p>" });
    check(
      "getText joins paragraphs with single newlines",
      input.getText() === "first\nsecond",
    );
    input.clear();
    check("clear empties the editor", input.getText() === "");
    input.dispose();
  }

  // --- Editable gate ---------------------------------------------------------------

  {
    const input = new PromptInput();
    const editor = editorElement(input);
    input.setEditable(false);
    check(
      "setEditable(false) lifts contenteditable",
      editor.getAttribute("contenteditable") === "false",
    );
    input.setEditable(true);
    check(
      "setEditable(true) restores contenteditable",
      editor.getAttribute("contenteditable") === "true",
    );
    input.dispose();
  }

  // --- Dispose -----------------------------------------------------------------------

  {
    const input = new PromptInput({ content: "<p>hello</p>" });
    document.body.appendChild(input.element);
    check(
      "a live editor renders its paragraph",
      editorElement(input)?.querySelector("p") !== null,
    );
    input.dispose();
    check(
      "dispose destroys the editor, removing its DOM from the container",
      editorElement(input) === null,
    );
    input.element.remove();
  }
});

if (failures.length > 0) {
  console.error(`prompt-input: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("prompt-input: all assertions passed");
process.exit(0);
