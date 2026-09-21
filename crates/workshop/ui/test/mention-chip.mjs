// The mention chip (src/parts/chatbox/mention-chip.ts) in jsdom: the
// configured Mention extension renamed to mentionNode with a vanilla-DOM
// NodeView pill. Covers: a mention node renders as a pill with icon
// slot, label, and a labelled remove button; the pill has the
// mention's rendered data-id, data-label, and data-mention-suggestion-char
// attributes; the label falls back to the
// id when no label is set; the chip is non-editable; the remove button
// deletes the node and leaves the surrounding text intact; getJSON
// serializes the node with type "mentionNode"; ChatBox registers the
// extension, so chips render and remove inside the real input. The chip
// model: a pill inserted with a kind has data-kind and one without
// omits it; kind, icon, preview, tone, and data survive getJSON, are
// null on a chip inserted without them, and are rebuilt by setContent
// from that JSON; data round-trips byte-for-byte; parsing the pill's
// rendered HTML (copy and paste) restores data-payload; renderChip
// (src/parts/chatbox/chip-view.ts) draws the same pill standalone. Runs
// under the shared leak check: a ChatBox that is never disposed
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
      export { ChatBox } from "./src/parts/chatbox/chat-box.ts";
      export { MentionChip } from "./src/parts/chatbox/mention-chip.ts";
      export { renderChip } from "./src/parts/chatbox/chip-view.ts";
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
const { lifecycle, ChatBox, MentionChip, renderChip, Editor, StarterKit } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// A bare editor over the same extensions ChatBox uses, so the chip
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

function mentionJson(editor) {
  const paragraph = editor.getJSON().content?.[0];
  return paragraph?.content?.find((node) => node.type === "mentionNode");
}

// A bare editor with one chip holding the given attrs at position 1.
function editorWithChip(attrs) {
  const editor = new Editor({
    element: document.createElement("div"),
    extensions: [StarterKit, MentionChip],
    content: "<p>x</p>",
  });
  editor.commands.insertContentAt(1, { type: "mentionNode", attrs });
  return editor;
}

// The full chip model, with a payload whose shape exercises nesting,
// unicode, and JSON-significant characters.
const FULL_CHIP = {
  id: "src/main.ts",
  label: "main.ts",
  kind: "file",
  icon: "file-code",
  preview: "pf://preview/1",
  tone: "expired",
  data: { path: "src/main.ts", grants: ["r\"w"], nested: { n: 1.5, ok: true, none: null }, s: "é\n" },
};

await assertNoLeaks(lifecycle, () => {
  // --- Render ---------------------------------------------------------------

  {
    const editor = createEditor();
    const chip = editor.view.dom.querySelector(".ws-mention-chip");
    check("a mention node renders as a pill inside the editor", chip !== null);
    check(
      "the pill shows the mention label",
      chip?.querySelector(".ws-mention-chip__label")?.textContent === "README.md",
    );
    check(
      "the pill includes an icon slot",
      chip?.querySelector(".ws-mention-chip__icon") !== null,
    );
    check(
      "the pill is non-editable",
      chip?.getAttribute("contenteditable") === "false",
    );
    check(
      "the pill includes a labelled remove button",
      chip?.querySelector('button.ws-mention-chip__remove[aria-label="Remove"]') !== null,
    );
    check(
      "the pill has the mention's rendered data attributes",
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
      editor.view.dom.querySelector(".ws-mention-chip__label")?.textContent === "src/main.ts",
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
    const button = editor.view.dom.querySelector(".ws-mention-chip__remove");
    button?.click();
    check(
      "the remove button deletes the mention node",
      editor.view.dom.querySelector(".ws-mention-chip") === null && !mentionInDoc(editor),
    );
    check(
      "the surrounding text survives the removal",
      editor.getText() === "before after",
    );
    editor.destroy();
  }

  // --- ChatBox registration -------------------------------------------------

  {
    const input = new ChatBox({
      content:
        '<p>look at <span data-type="mentionNode" data-id="README.md" data-label="README.md"></span> please</p>',
    });
    check(
      "ChatBox renders a mention node as a pill",
      input.element.querySelector(".ws-mention-chip") !== null,
    );
    input.element.querySelector(".ws-mention-chip__remove")?.click();
    check(
      "the remove button deletes the chip inside ChatBox",
      input.element.querySelector(".ws-mention-chip") === null,
    );
    input.dispose();
  }

  // --- Chip model: kind on the pill ------------------------------------------------

  {
    const editor = editorWithChip({ id: "src/main.ts", label: "main.ts", kind: "file" });
    check(
      "a pill inserted with a kind has data-kind",
      editor.view.dom.querySelector(".ws-mention-chip")?.getAttribute("data-kind") === "file",
    );
    editor.destroy();
  }

  {
    const editor = editorWithChip({ id: "src/main.ts", label: "main.ts" });
    check(
      "a pill inserted without a kind omits data-kind",
      editor.view.dom.querySelector(".ws-mention-chip")?.hasAttribute("data-kind") === false,
    );
    editor.destroy();
  }

  // --- Chip model: the extended attrs survive getJSON -----------------------------

  {
    const editor = editorWithChip(FULL_CHIP);
    const mention = mentionJson(editor);
    check(
      "kind, icon, preview, and tone survive getJSON",
      mention?.attrs?.kind === "file" &&
        mention?.attrs?.icon === "file-code" &&
        mention?.attrs?.preview === "pf://preview/1" &&
        mention?.attrs?.tone === "expired",
    );
    check(
      "data round-trips through getJSON byte-for-byte",
      JSON.stringify(mention?.attrs?.data) === JSON.stringify(FULL_CHIP.data),
    );
    // setContent from the serialized JSON: the round trip is the persisted
    // draft's path back into a live editor.
    const rebuilt = new Editor({
      element: document.createElement("div"),
      extensions: [StarterKit, MentionChip],
      content: editor.getJSON(),
    });
    const again = mentionJson(rebuilt);
    check(
      "setContent from the JSON rebuilds the extended attrs",
      again?.attrs?.kind === "file" &&
        again?.attrs?.icon === "file-code" &&
        again?.attrs?.preview === "pf://preview/1" &&
        again?.attrs?.tone === "expired" &&
        JSON.stringify(again?.attrs?.data) === JSON.stringify(FULL_CHIP.data),
    );
    check(
      "the rebuilt pill renders its kind and tone",
      rebuilt.view.dom.querySelector(".ws-mention-chip")?.getAttribute("data-kind") === "file" &&
        rebuilt.view.dom.querySelector(".ws-mention-chip")?.getAttribute("data-tone") === "expired",
    );
    rebuilt.destroy();
    editor.destroy();
  }

  {
    const editor = editorWithChip({ id: "README.md", label: "README.md" });
    const mention = mentionJson(editor);
    check(
      "kind, icon, preview, tone, and data are absent from a chip inserted without them",
      mention !== undefined &&
        mention.attrs?.kind == null &&
        mention.attrs?.icon == null &&
        mention.attrs?.preview == null &&
        mention.attrs?.tone == null &&
        mention.attrs?.data == null,
    );
    editor.destroy();
  }

  // --- Chip model: the rendered HTML parses back (copy and paste) -----------------

  {
    const editor = editorWithChip(FULL_CHIP);
    // getHTML runs the schema's renderHTML - the clipboard serializer's
    // path - and setContent from that HTML runs parseHTML, the paste path.
    const html = editor.getHTML();
    check(
      "the rendered pill HTML includes a JSON data-payload and the model attributes",
      html.includes('data-kind="file"') &&
        html.includes('data-icon="file-code"') &&
        html.includes('data-preview="pf://preview/1"') &&
        html.includes('data-tone="expired"') &&
        html.includes("data-payload="),
    );
    const pasted = new Editor({
      element: document.createElement("div"),
      extensions: [StarterKit, MentionChip],
      content: html,
    });
    const mention = mentionJson(pasted);
    check(
      "parsing the pill's HTML restores the model attrs and data-payload",
      mention?.attrs?.id === "src/main.ts" &&
        mention?.attrs?.kind === "file" &&
        mention?.attrs?.icon === "file-code" &&
        mention?.attrs?.preview === "pf://preview/1" &&
        mention?.attrs?.tone === "expired" &&
        JSON.stringify(mention?.attrs?.data) === JSON.stringify(FULL_CHIP.data),
    );
    pasted.destroy();
    editor.destroy();
  }

  {
    const plain = new Editor({
      element: document.createElement("div"),
      extensions: [StarterKit, MentionChip],
      content:
        '<p><span data-type="mentionNode" data-id="a" data-label="a" data-payload="not json"></span></p>',
    });
    check(
      "an unparseable data-payload parses as null rather than throwing",
      mentionJson(plain)?.attrs?.data === null,
    );
    plain.destroy();
  }

  // --- renderChip: the standalone pill --------------------------------------------

  {
    const pill = renderChip(FULL_CHIP);
    check(
      "renderChip draws the pill with icon, label, and remove button",
      pill.classList.contains("ws-mention-chip") &&
        pill.querySelector(".ws-mention-chip__icon svg") !== null &&
        pill.querySelector(".ws-mention-chip__label")?.textContent === "main.ts" &&
        pill.querySelector('button.ws-mention-chip__remove[aria-label="Remove"]') !== null,
    );
    check(
      "renderChip stamps data-kind and data-tone from the chip",
      pill.getAttribute("data-kind") === "file" && pill.getAttribute("data-tone") === "expired",
    );
    const bare = renderChip({ id: "x", label: "x", data: null });
    check(
      "renderChip leaves data-kind and data-tone off a chip without them",
      !bare.hasAttribute("data-kind") && !bare.hasAttribute("data-tone"),
    );
    const byExtension = renderChip({ id: "notes.md", label: "notes.md", data: null });
    check(
      "renderChip picks the icon from the label's extension when none is named",
      byExtension.querySelector(".ws-mention-chip__icon svg")?.outerHTML !==
        bare.querySelector(".ws-mention-chip__icon svg")?.outerHTML,
    );
    const named = renderChip({ id: "notes.md", label: "notes.md", icon: "folder", data: null });
    check(
      "a named icon overrides the extension map",
      named.querySelector(".ws-mention-chip__icon svg")?.outerHTML !==
        byExtension.querySelector(".ws-mention-chip__icon svg")?.outerHTML,
    );
    check(
      "an unknown named icon falls back to the extension map",
      renderChip({ id: "notes.md", label: "notes.md", icon: "no-such-icon", data: null })
        .querySelector(".ws-mention-chip__icon svg")?.outerHTML ===
        byExtension.querySelector(".ws-mention-chip__icon svg")?.outerHTML,
    );
  }
});

if (failures.length > 0) {
  console.error(`ws-mention-chip: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ws-mention-chip: all assertions passed");
process.exit(0);
