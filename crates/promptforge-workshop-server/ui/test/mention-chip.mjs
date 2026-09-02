// The mention chip (src/ui/workshop/mention-chip.ts) in jsdom: the
// configured Mention extension renamed to mentionNode with a vanilla-DOM
// NodeView pill. Covers: a mention node renders as a pill with icon
// slot, label, and a labelled remove button; the pill carries the
// mention's rendered data-id, data-label, and data-mention-suggestion-char
// attributes; the label falls back to the
// id when no label is set; the chip is non-editable; the remove button
// deletes the node and leaves the surrounding text intact; getJSON
// serializes the node with type "mentionNode"; PromptInput registers the
// extension, so chips render and remove inside the real input. Runs
// under the shared leak check: a PromptInput that is never disposed
// fails.
// Run: node test/mention-chip.mjs
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
// supplies the requestAnimationFrame ProseMirror schedules with.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);

const bundlePath = path.join(os.tmpdir(), "promptforge-mention-chip-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, PromptInput, MentionChip, Editor, StarterKit } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// A bare editor over the same extensions PromptInput uses, so the chip
// mechanics are pinned directly against the extension.
function createEditor() {
  const element = document.createElement("div");
  const editor = new Editor({
    element,
    extensions: [StarterKit, MentionChip],
    content: "<p>before after</p>",
  });
  editor.commands.insertContentAt(7, {
    type: "mentionNode",
    attrs: { id: "README.md", label: "README.md" },
  });
  return editor;
}

function mentionInDoc(editor) {
  let found = false;
  editor.state.doc.descendants((node) => {
    if (node.type.name === "mentionNode") found = true;
    return !found;
  });
  return found;
}

await assertNoLeaks(lifecycle, () => {
  // --- Render ---------------------------------------------------------------

  {
    const editor = createEditor();
    const chip = editor.view.dom.querySelector(".mention-chip");
    check("a mention node renders as a pill inside the editor", chip !== null);
    check(
      "the pill shows the mention label",
      chip?.querySelector(".mention-chip__label")?.textContent === "README.md",
    );
    check(
      "the pill carries an icon slot",
      chip?.querySelector(".mention-chip__icon") !== null,
    );
    check(
      "the pill is non-editable",
      chip?.getAttribute("contenteditable") === "false",
    );
    check(
      "the pill carries a labelled remove button",
      chip?.querySelector('button.mention-chip__remove[aria-label="Remove"]') !== null,
    );
    check(
      "the pill carries the mention's rendered data attributes",
      chip?.getAttribute("data-id") === "README.md" &&
        chip?.getAttribute("data-label") === "README.md" &&
        chip?.getAttribute("data-mention-suggestion-char") === "@",
    );
    editor.destroy();
  }

  // --- Label fallback ---------------------------------------------------------

  {
    const editor = new Editor({
      element: document.createElement("div"),
      extensions: [StarterKit, MentionChip],
      content: "<p>x</p>",
    });
    editor.commands.insertContentAt(1, {
      type: "mentionNode",
      attrs: { id: "src/main.ts" },
    });
    check(
      "a mention without a label falls back to its id",
      editor.view.dom.querySelector(".mention-chip__label")?.textContent === "src/main.ts",
    );
    editor.destroy();
  }

  // --- Serialization ----------------------------------------------------------

  {
    const editor = createEditor();
    const json = editor.getJSON();
    const paragraph = json.content?.[0];
    const mention = paragraph?.content?.find((node) => node.type === "mentionNode");
    check(
      "getJSON serializes the mention with the mentionNode type",
      mention !== undefined &&
        mention.attrs?.id === "README.md" &&
        mention.attrs?.label === "README.md",
    );
    editor.destroy();
  }

  // --- Remove -----------------------------------------------------------------

  {
    const editor = createEditor();
    const button = editor.view.dom.querySelector(".mention-chip__remove");
    button?.click();
    check(
      "the remove button deletes the mention node",
      editor.view.dom.querySelector(".mention-chip") === null && !mentionInDoc(editor),
    );
    check(
      "the surrounding text survives the removal",
      editor.getText() === "before after",
    );
    editor.destroy();
  }

  // --- PromptInput registration -------------------------------------------------

  {
    const input = new PromptInput({
      content:
        '<p>look at <span data-type="mentionNode" data-id="README.md" data-label="README.md"></span> please</p>',
    });
    check(
      "PromptInput renders a mention node as a pill",
      input.element.querySelector(".mention-chip") !== null,
    );
    input.element.querySelector(".mention-chip__remove")?.click();
    check(
      "the remove button deletes the chip inside PromptInput",
      input.element.querySelector(".mention-chip") === null,
    );
    input.dispose();
  }
});

if (failures.length > 0) {
  console.error(`mention-chip: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("mention-chip: all assertions passed");
process.exit(0);
