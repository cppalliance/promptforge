// The chat box (src/parts/chatbox/chat-box.ts) in jsdom: a Tiptap/
// ProseMirror editor framed as the chat box, with its mic and send
// buttons on the bar. Covers: the editor mounts inside the framed
// container with an accessible editable region; the placeholder
// decorates the empty paragraph and lifts once content lands; Enter
// emits `send` while an IME-composition Enter and Shift+Enter do not
// (Shift+Enter inserts a hard break); the box height tracks content
// clamped between the min/max tokens (jsdom reports scrollHeight 0, so
// the test stubs it to drive the clamp, and pins the exported clamp
// directly); getText returns paragraphs and breaks as single newlines;
// clear empties; update({ editable }) toggles contenteditable; the box
// registers a prosemirror text-control adapter through the injected registrar
// whose canUndo/canRedo track the ProseMirror history plugin's depth;
// setTentativeRange decorates each dictation take's tentative span, maps it
// through later edits, and clears it, never changing the document text;
// a transient replaceRange stays out of undo history while any other is
// its own undo step, so each landed dictation undoes whole and alone and
// never merges with an edit typed just before or after it; restoreRange
// puts a captured selection's pills and paragraph breaks back outside
// undo history, so undoing a dictation over them brings them back;
// dispose destroys the editor. The contract: defaults, data-* state
// mirrors (variant, editable, action, mic), the send button's three
// states, the mic button's rendering per state, the controls slot, the
// attachments strip, update() as a DOM no-op for unchanged props, and
// the `send` event's mentions. The handle's persistence surface:
// insertMention places a pill plus one trailing space at the cursor;
// serialize/restore round-trips text, pills, and each pill's payload
// byte-for-byte and paints attachments into the strip; restore with a
// missing or unknown `v` leaves the box unchanged. The typeahead seams:
// the default stub source lists its three entries when `@` is typed;
// an injected mentionSource replaces the stub and the popup lists its
// items; the source receives the ProseMirror plugin's AbortSignal, which
// fires when a newer keystroke arrives; an older query resolving after a
// newer one does not overwrite the newer results; `/` is plain text with no
// popup (commandSource's default is stored, not wired). The static
// renderer (src/parts/chatbox/chat-box-view.ts): renderDraft turns a
// SerializedDraft with text, one inline pill, and one attachment into
// the expected read-only DOM. Runs under the shared leak check: a
// ChatBox that is never disposed fails.
// The round action button: the mic over an empty box, the send arrow once
// there is text, Stop while the agent generates (with the placeholder "Add a
// follow-up"), the agent mode picking the send and Stop fill; a recording mic
// stays the mic; Enter ignores auto-repeat and Ctrl+Enter sends. The
// stylesheet's side - the card, the editor, the button's fills - is read from
// the source text.
// Run: node test/chat-box.mjs
import { readFile, writeFile } from "node:fs/promises";
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
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { ChatBox, clampPromptInputHeight, stubMentionSource } from "./src/parts/chatbox/chat-box.ts";
      export { renderDraft } from "./src/parts/chatbox/chat-box-view.ts";
      export { TEXT_CONTROL_SERVICE } from "@workshop/platform/text-control-service";
      export { getService } from "@workshop/platform/service-registry";
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
// The text-control service probes these constructor globals when it
// classifies the focused element.
globalThis.Element = dom.window.Element;
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.HTMLInputElement = dom.window.HTMLInputElement;
globalThis.HTMLTextAreaElement = dom.window.HTMLTextAreaElement;
globalThis.Node = dom.window.Node;
// The ProseMirror suggestion plugin's managed mount reads the DOMRect global.
globalThis.DOMRect = dom.window.DOMRect;
// Tiptap's focus command schedules with the bare globals.
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window);
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window);
// jsdom has no layout: a focused editor's scroll-to-selection measures
// the cursor through range geometry, so stub it to zero rects.
const zeroRect = { x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, toJSON: () => ({}) };
dom.window.Range.prototype.getClientRects = () => [];
dom.window.Range.prototype.getBoundingClientRect = () => zeroRect;

const bundlePath = path.join(os.tmpdir(), "promptforge-chat-box-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  lifecycle,
  ChatBox,
  clampPromptInputHeight,
  stubMentionSource,
  renderDraft,
  TEXT_CONTROL_SERVICE,
  getService,
} = await import(pathToFileURL(bundlePath).href);

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
  return input.element.querySelector(".ws-prompt-input__editor");
}

function frameElement(input) {
  return input.element.querySelector(".ws-prompt-input");
}

// The one round button: the mic while the box is empty, the send arrow once it
// has text, Stop while the agent generates. Both names reach the same element,
// so each test reads as the state it is exercising.
function actionButton(input) {
  return input.element.querySelector(".ws-agent-session__action");
}
const micButton = actionButton;
const sendButton = actionButton;

// The ProseMirror suggestion plugin debounces its item fetch (the
// component configures 50 to 100 ms), so a typeahead assertion waits past
// that window plus the mount's computePosition before reading the popup.
function settle() {
  return new Promise((resolve) => setTimeout(resolve, 160));
}

// A mounted box whose editor the test can drive: Tiptap stamps the
// Editor on the view DOM (dom.editor), and the suggestion session only
// activates for a focused, connected editor.
function mountedBox(props = {}, sink = () => {}) {
  const input = new ChatBox(props, sink);
  document.body.appendChild(input.element);
  const editor = editorElement(input).editor;
  editor.commands.focus();
  return { input, editor };
}

// insertContent dispatches the same transaction typing would.
async function typeText(editor, text) {
  editor.commands.insertContent(text);
  await settle();
}

function popup() {
  return document.body.querySelector(".ws-typeahead-popup");
}

function popupLabels() {
  return [...(popup()?.querySelectorAll(".ws-typeahead-popup__item") ?? [])].map(
    (item) => item.querySelector(".ws-typeahead-popup__label")?.textContent ?? item.textContent,
  );
}

// A source whose every call is recorded and resolves only when the test
// says so, for the abort and stale-result assertions.
function deferredSource() {
  const calls = [];
  const source = (query, signal) =>
    new Promise((resolve) => {
      calls.push({ query, signal, resolve });
    });
  source.calls = calls;
  return source;
}

// A sink that records every event and counts the sends, standing in for
// the onSubmit callback the box used to take.
function recordingSink() {
  const events = [];
  const sink = (event) => {
    events.push(event);
  };
  sink.events = events;
  sink.sends = () => events.filter((event) => event.type === "send").length;
  return sink;
}

await assertNoLeaks(lifecycle, async () => {
  // --- Mount ----------------------------------------------------------------

  {
    const input = new ChatBox();
    const editor = editorElement(input);
    check(
      "the editor mounts a ProseMirror region inside the framed container on the bar",
      input.element.classList.contains("ws-agent-session__bar") &&
        frameElement(input) !== null &&
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
    const input = new ChatBox({ placeholder: "Message the agent" });
    const empty = editorElement(input).querySelector("p");
    check(
      "the empty paragraph has the placeholder decoration",
      empty !== null &&
        empty.classList.contains("is-editor-empty") &&
        empty.getAttribute("data-placeholder") === "Message the agent",
    );
    const filled = new ChatBox({ content: "<p>hello</p>" });
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
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>hello</p>" }, sink);
    const editor = editorElement(input);
    pressEnter(editor);
    check("Enter emits send", sink.sends() === 1);
    check(
      "the send event holds the text untrimmed with empty mentions and attachments",
      sink.events[0]?.text === "hello" &&
        Array.isArray(sink.events[0]?.mentions) &&
        sink.events[0].mentions.length === 0 &&
        Array.isArray(sink.events[0]?.attachments) &&
        sink.events[0].attachments.length === 0,
    );
    check(
      "a submitting Enter leaves the text untouched",
      input.getText() === "hello",
    );
    input.dispose();
  }

  {
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>hello</p>" }, sink);
    const editor = editorElement(input);
    // A full composition session: ProseMirror tracks composing state
    // from compositionstart, so the committing Enter is inert end to end.
    editor.dispatchEvent(new dom.window.CompositionEvent("compositionstart", { bubbles: true }));
    pressEnter(editor, { isComposing: true });
    check(
      "an Enter committing an IME composition does not submit",
      sink.sends() === 0,
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
      sink.sends() === 0,
    );
    check(
      "an Enter flagged isComposing is claimed, not split into a paragraph",
      input.getText() === "hello",
    );
    input.dispose();
  }

  {
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>hello</p>" }, sink);
    const editor = editorElement(input);
    pressEnter(editor, { shiftKey: true });
    check("Shift+Enter does not submit", sink.sends() === 0);
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
    clampPromptInputHeight(150, 22, 240) === 150,
  );
  check(
    "the clamp holds heights at the max token",
    clampPromptInputHeight(500, 22, 240) === 240,
  );
  check(
    "the clamp lifts heights to the min token",
    clampPromptInputHeight(10, 22, 240) === 22,
  );

  {
    const input = new ChatBox({ content: "<p>hello</p>" });
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
      "the box height clamps at the max token, 240px by default",
      editor.style.height === "240px",
    );
    measured = 10;
    input.syncHeight();
    check(
      "the box height clamps at the min token, one 22px line by default",
      editor.style.height === "22px",
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
    const input = new ChatBox({ content: "<p>first</p><p>second</p>" });
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
    const input = new ChatBox();
    const editor = editorElement(input);
    input.update({ editable: false });
    check(
      "update({ editable: false }) lifts contenteditable",
      editor.getAttribute("contenteditable") === "false",
    );
    input.update({ editable: true });
    check(
      "update({ editable: true }) restores contenteditable",
      editor.getAttribute("contenteditable") === "true",
    );
    input.dispose();
  }

  // --- The dictation target seam (SttInputTarget) ----------------------------

  {
    const input = new ChatBox();
    input.setText("ab");
    check("setText loads plain text", input.getText() === "ab");
    input.setSelection(2, 2);
    const middle = input.insertionContext();
    check(
      "insertionContext captures a mid-word cursor with no composition prefix",
      middle.range.start === 2 &&
        middle.range.end === 2 &&
        middle.original === "" &&
        middle.compositionPrefix === "",
    );
    input.replaceRange(2, 2, "X");
    check("replaceRange splices at the cursor", input.getText() === "aXb");
    const afterInsert = input.insertionContext().range;
    check(
      "replaceRange leaves the cursor after the inserted text",
      afterInsert.start === 3 && afterInsert.end === 3,
    );
    input.replaceRange(1, 4, "");
    check("replaceRange with empty text deletes the range", input.getText() === "");
    input.setText("line one\nline two");
    check(
      "setText writes one paragraph per newline",
      input.getText() === "line one\nline two" &&
        editorElement(input).querySelectorAll("p").length === 2,
    );
    input.dispose();
  }

  {
    const input = new ChatBox();
    input.setText("First test alpha");
    const append = input.insertionContext();
    check(
      "insertionContext captures a ProseMirror append separator",
      append.range.start === append.range.end &&
        append.range.end === 17 &&
        append.original === "" &&
        append.compositionPrefix === " ",
    );
    input.replaceRange(append.range.start, append.range.end, " ");
    check(
      "a captured ProseMirror composition prefix is immutable",
      append.compositionPrefix === " ",
    );
    input.setText("First test alpha ");
    check(
      "insertionContext preserves existing ProseMirror trailing whitespace",
      input.insertionContext().compositionPrefix === "",
    );
    input.setText("First test alpha");
    input.setSelection(7, 11);
    const replacement = input.insertionContext();
    check(
      "insertionContext captures selected ProseMirror text without a separator",
      replacement.range.start === 7 &&
        replacement.range.end === 11 &&
        replacement.original === "test" &&
        replacement.compositionPrefix === "",
    );
    input.dispose();
  }

  // --- Newlines cross the target seam ---------------------------------------------

  {
    const input = new ChatBox();
    input.setText("a\n\nb");
    check(
      "setText writes an empty paragraph for an empty line",
      input.getText() === "a\n\nb" &&
        editorElement(input).querySelectorAll("p").length === 3,
    );
    input.setText("ab");
    input.setSelection(2, 2);
    input.replaceRange(2, 2, "x\ny");
    check(
      "replaceRange splices a newline as a hard break inside the paragraph",
      input.getText() === "ax\nyb" &&
        editorElement(input).querySelectorAll("p").length === 1,
    );
    // The take's splice math (TakeState.length in stt.ts) holds only while
    // every inserted character, newline included, occupies one position.
    const afterNewline = input.insertionContext().range;
    check(
      "a spliced newline occupies one position, keeping the take's length arithmetic",
      afterNewline.start === 5 && afterNewline.end === 5,
    );
    input.replaceRange(2, 5, "");
    check(
      "deleting the spliced range restores the pre-take text",
      input.getText() === "ab",
    );
    input.dispose();
  }

  // --- Dictation's tentative tail is a decoration, not text ----------------------

  {
    const input = new ChatBox();
    input.setText("ask not what your");
    const before = JSON.stringify(input.serialize());
    const marks = () =>
      [...editorElement(input).querySelectorAll(".ws-stt-tentative")].map((mark) => mark.textContent);
    const unchanged = () =>
      input.getText() === "ask not what your" && JSON.stringify(input.serialize()) === before;
    // ProseMirror positions: the paragraph opens at 0, so " your" spans 13..18.
    input.setTentativeRange(1, { from: 13, to: 18 });
    check("a tentative range decorates exactly its span", marks().join("|") === " your");
    check("the tentative decoration leaves the document text unchanged", unchanged());
    input.setTentativeRange(2, { from: 1, to: 4 });
    check("each take owns its own tentative range", marks().join("|") === "ask| your");
    input.setTentativeRange(1, { from: 8, to: 13 });
    check("a take's next range replaces its previous one", marks().join("|") === "ask| what");
    input.replaceRange(1, 1, "so ");
    check("both ranges follow an edit before them", marks().join("|") === "ask| what");
    input.replaceRange(1, 4, "");
    input.setTentativeRange(1, null);
    input.setTentativeRange(2, null);
    check("clearing every take's range removes the decoration", marks().length === 0);
    check("clearing the decoration leaves the document text unchanged", unchanged());
    input.setTentativeRange(1, { from: 30, to: 40 });
    check("a range outside the document is ignored", marks().length === 0 && unchanged());
    input.dispose();
  }

  // --- The two locks compose on one contenteditable -----------------------------

  {
    const input = new ChatBox();
    const editor = editorElement(input);
    const frame = frameElement(input);
    input.setReadOnly(true);
    check(
      "setReadOnly locks the editor and marks the frame",
      editor.getAttribute("contenteditable") === "false" &&
        frame.classList.contains("ws-stt-input--recording"),
    );
    input.update({ editable: false });
    input.setReadOnly(false);
    check(
      "lifting the take lock under a closed gate stays non-editable",
      editor.getAttribute("contenteditable") === "false" &&
        !frame.classList.contains("ws-stt-input--recording"),
    );
    input.setReadOnly(true);
    input.update({ editable: true });
    check(
      "the gate reopening under a live take lock stays non-editable",
      editor.getAttribute("contenteditable") === "false",
    );
    input.setReadOnly(false);
    check(
      "lifting the last lock reopens the editor",
      editor.getAttribute("contenteditable") === "true",
    );
    input.dispose();
  }

  // --- Enter submits while read-only --------------------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>hello</p>" }, sink);
    input.setReadOnly(true);
    pressEnter(editorElement(input));
    check(
      "Enter emits send while the box is read-only (a live take)",
      sink.sends() === 1,
    );
    check(
      "the read-only submitting Enter leaves the text untouched",
      input.getText() === "hello",
    );
    input.dispose();
  }

  // --- Placeholder dynamics --------------------------------------------------------

  {
    let label = "first";
    const input = new ChatBox({ placeholder: () => label });
    check(
      "a function placeholder is evaluated for the decoration",
      editorElement(input).querySelector("p")?.getAttribute("data-placeholder") === "first",
    );
    label = "second";
    input.update({ editable: false });
    check(
      "the placeholder re-evaluates on the gate flip",
      editorElement(input).querySelector("p")?.getAttribute("data-placeholder") === "second",
    );
    check(
      "the placeholder still shows while non-editable",
      editorElement(input).querySelector("p")?.classList.contains("is-editor-empty") === true,
    );
    input.dispose();
  }

  // --- The text-control adapter (Edit menu routing) ----------------------------

  {
    const textControls = getService(TEXT_CONTROL_SERVICE);
    const input = new ChatBox({ textControls: textControls.register.bind(textControls) });
    document.body.appendChild(input.element);
    input.focus();
    // Tiptap defers the DOM focus to the next animation frame.
    await new Promise((resolve) => globalThis.requestAnimationFrame(resolve));
    const active = textControls.active;
    check(
      "the prompt registers a prosemirror text-control adapter through the injected registrar",
      active !== null && active.kind === "prosemirror",
    );
    check(
      "a fresh prompt reports an empty undo and redo history",
      active !== null && active.canUndo() === false && active.canRedo() === false,
    );
    input.setText("hello");
    check("an edit deepens the adapter's undo history", active !== null && active.canUndo() === true);
    textControls.undo();
    check("routing undo through the service reverts the edit", input.getText() === "");
    check("the reverted edit reports redo depth", active !== null && active.canRedo() === true);
    textControls.redo();
    check("routing redo through the service replays the edit", input.getText() === "hello");
    input.dispose();
    check("disposing the prompt unregisters its adapter", textControls.active === null);
    input.element.remove();
  }

  // --- Each landed dictation is one undo step --------------------------------------

  {
    const textControls = getService(TEXT_CONTROL_SERVICE);
    const input = new ChatBox({ textControls: textControls.register.bind(textControls) });
    document.body.appendChild(input.element);
    input.focus();
    await new Promise((resolve) => globalThis.requestAnimationFrame(resolve));
    const active = textControls.active;
    input.replaceRange(1, 1, "draft", { transient: true });
    check(
      "a transient replace lands its text but leaves undo history empty",
      input.getText() === "draft" && active !== null && active.canUndo() === false,
    );
    input.replaceRange(1, 6, "", { transient: true });
    input.setText("Note");
    // Two dictations back to back, as the take registry writes them:
    // transient interims, a transient restore of the original, then one
    // tracked write of the landed text.
    input.replaceRange(5, 5, " ask", { transient: true });
    input.replaceRange(5, 9, " ask not", { transient: true });
    input.replaceRange(5, 13, "", { transient: true });
    input.replaceRange(5, 5, " ask not what");
    input.replaceRange(18, 18, " more", { transient: true });
    input.replaceRange(18, 23, "", { transient: true });
    input.replaceRange(18, 18, " and more");
    check("both dictations land", input.getText() === "Note ask not what and more");
    textControls.undo();
    check("one undo removes only the latest dictation", input.getText() === "Note ask not what");
    textControls.undo();
    check(
      "the next undo removes the earlier dictation whole, not its interim writes",
      input.getText() === "Note",
    );
    input.dispose();
    input.element.remove();
  }

  {
    const { input, editor } = mountedBox();
    input.setText("Note");
    editor.commands.insertContent(" typed");
    input.replaceRange(11, 11, " spoken");
    editor.commands.undo();
    check(
      "an edit typed just before a landed replace stays when the replace is undone",
      input.getText() === "Note typed",
    );
    input.dispose();
    input.element.remove();
  }

  {
    const { input, editor } = mountedBox();
    input.setText("Note");
    input.replaceRange(5, 5, " spoken");
    editor.commands.insertContent(" typed");
    editor.commands.undo();
    check(
      "an edit typed just after a landed replace undoes alone",
      input.getText() === "Note spoken",
    );
    editor.commands.undo();
    check("the landed replace then undoes alone", input.getText() === "Note");
    input.dispose();
    input.element.remove();
  }

  // --- A dictation over pills and paragraphs undoes back to them -------------------

  {
    const { input, editor } = mountedBox({
      content:
        '<p>see <span data-type="mentionNode" data-id="src/main.ts" data-label="main.ts"></span> now</p><p>then</p>',
    });
    const before = JSON.stringify(input.serialize().doc);
    // ProseMirror positions: the pill sits at 5, the second paragraph's "then" spans 12..16.
    input.setSelection(5, 14);
    const context = input.insertionContext();
    // The take registry's writes: a whole first interim, a patch, the
    // restore of the captured selection, then the one tracked final.
    input.replaceRange(5, 14, "spoken", { transient: true });
    input.replaceRange(11, 11, " words", { transient: true });
    input.restoreRange(5, 17, context.content);
    check(
      "a restore puts the captured pill and paragraph break back outside undo history",
      JSON.stringify(input.serialize().doc) === before &&
        editor.can().undo() === false &&
        editor.state.selection.from === 14,
    );
    input.replaceRange(5, 14, "spoken words");
    check("the final replaces the restored selection", input.getText() === "see spoken wordsen");
    editor.commands.undo();
    check(
      "one undo brings back the pill and the paragraph break the dictation replaced",
      JSON.stringify(input.serialize().doc) === before,
    );
    input.dispose();
    input.element.remove();
  }

  {
    const textControls = getService(TEXT_CONTROL_SERVICE);
    const input = new ChatBox();
    document.body.appendChild(input.element);
    input.focus();
    await new Promise((resolve) => globalThis.requestAnimationFrame(resolve));
    check(
      "a box built without a registrar makes no service-registry registration",
      textControls.active === null,
    );
    input.dispose();
    input.element.remove();
  }

  // --- Dispose -----------------------------------------------------------------------

  {
    const input = new ChatBox({ content: "<p>hello</p>" });
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

  // --- The contract: defaults and data-* state mirrors ---------------------------

  {
    const input = new ChatBox();
    const frame = frameElement(input);
    const mic = micButton(input);
    const send = sendButton(input);
    check(
      "props read back the defaults",
      input.props.editable === true &&
        input.props.action === "send" &&
        input.props.mic === "idle" &&
        input.props.mode === "agent" &&
        input.props.variant === "expanded",
    );
    check(
      "the root has data-variant=expanded with the prop absent",
      input.element.getAttribute("data-variant") === "expanded",
    );
    check(
      "the frame mirrors the default editable state",
      frame.getAttribute("data-editable") === "true" &&
        editorElement(input).getAttribute("contenteditable") === "true",
    );
    check(
      "the editable region has the default accessible name",
      editorElement(input).getAttribute("aria-label") === "Message",
    );
    check(
      "the default placeholder is empty",
      editorElement(input).querySelector("p")?.getAttribute("data-placeholder") === "",
    );
    check(
      "an empty box's one round button is the mic, enabled, with its accessible name, tooltip, and icon",
      mic !== null &&
        mic.getAttribute("data-state") === "mic" &&
        mic.getAttribute("data-action") === "send" &&
        mic.getAttribute("data-mic") === "idle" &&
        mic.getAttribute("data-mode") === "agent" &&
        mic.type === "button" &&
        mic.disabled === false &&
        mic.getAttribute("aria-label") === "Push to talk" &&
        mic.getAttribute("aria-pressed") === "false" &&
        mic.title === "Voice Input (Ctrl+Shift+Space)" &&
        mic.querySelector("svg") !== null,
    );
    check("the box has one action button and no separate mic", input.element.querySelectorAll("button").length === 1);
    input.setText("hello");
    check(
      "text turns the button into the send arrow: enabled, not aria-disabled, named Send",
      send.getAttribute("data-state") === "send" &&
        send.disabled === false &&
        send.getAttribute("aria-disabled") === "false" &&
        send.getAttribute("aria-label") === "Send" &&
        send.title === "Send" &&
        !send.hasAttribute("aria-pressed") &&
        send.querySelector("svg") !== null,
    );
    input.clear();
    check("emptying the box brings the mic back", send.getAttribute("data-state") === "mic");
    const strip = frame.querySelector(".ws-prompt-input__attachments");
    check(
      "an empty attachments strip sits inside the frame before the editor",
      strip !== null &&
        strip.childElementCount === 0 &&
        strip.parentElement === frame &&
        strip.nextElementSibling === editorElement(input),
    );
    check(
      "without controls the action button sits on the bar after the frame",
      mic.parentElement === input.element && frame.nextElementSibling === mic,
    );
    input.dispose();
  }

  {
    const input = new ChatBox({ variant: "expanded", ariaLabel: "Ask" });
    check(
      "an explicit variant and aria label render",
      input.element.getAttribute("data-variant") === "expanded" &&
        editorElement(input).getAttribute("aria-label") === "Ask",
    );
    input.dispose();
  }

  // --- data-editable is the effective state of both locks ------------------------

  {
    const input = new ChatBox();
    const frame = frameElement(input);
    const states = [frame.getAttribute("data-editable")];
    input.setReadOnly(true);
    states.push(frame.getAttribute("data-editable"));
    input.setReadOnly(false);
    states.push(frame.getAttribute("data-editable"));
    check(
      "data-editable reads true, false, true across a take lock",
      states.join(",") === "true,false,true",
    );
    input.update({ editable: false });
    check(
      "data-editable reads false under a closed gate",
      frame.getAttribute("data-editable") === "false" && input.props.editable === false,
    );
    input.setReadOnly(true);
    input.update({ editable: true });
    check(
      "data-editable stays false while a take lock outlives the gate",
      frame.getAttribute("data-editable") === "false" && input.props.editable === true,
    );
    input.dispose();
  }

  // --- The send button's states follow update() ---------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>draft</p>" }, sink);
    const send = sendButton(input);
    const editor = editorElement(input);
    send.click();
    check("a click on the send button emits send", sink.sends() === 1);

    input.update({ action: "send-blocked" });
    check(
      "send-blocked renders aria-disabled but stays clickable",
      send.getAttribute("data-action") === "send-blocked" &&
        send.disabled === false &&
        send.getAttribute("aria-disabled") === "true" &&
        input.props.action === "send-blocked",
    );
    send.click();
    check("a click while send-blocked still emits send", sink.sends() === 2);
    pressEnter(editor);
    check("Enter while send-blocked still emits send", sink.sends() === 3);

    input.update({ action: "idle" });
    check(
      "idle disables the send button",
      send.getAttribute("data-action") === "idle" &&
        send.disabled === true &&
        send.getAttribute("aria-disabled") === "false",
    );
    send.click();
    pressEnter(editor);
    check("idle is silent: neither a click nor Enter emits", sink.sends() === 3);

    input.update({ action: "send" });
    check(
      "returning to send re-enables the button",
      send.getAttribute("data-action") === "send" && send.disabled === false,
    );
    pressEnter(editor);
    check("Enter after returning to send emits again", sink.sends() === 4);
    input.dispose();
  }

  // --- Stop: the agent is generating --------------------------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({ placeholder: "Plan, Build" }, sink);
    const button = actionButton(input);
    const editor = editorElement(input);
    check(
      "before generating the placeholder is the owner's",
      editor.querySelector("p")?.getAttribute("data-placeholder") === "Plan, Build",
    );
    input.update({ action: "stop" });
    check(
      "stop turns the button into Stop, even with an empty box",
      button.getAttribute("data-state") === "stop" &&
        button.getAttribute("data-action") === "stop" &&
        button.disabled === false &&
        button.getAttribute("aria-label") === "Stop" &&
        button.title === "Stop (Ctrl+Shift+Backspace)" &&
        !button.hasAttribute("aria-pressed") &&
        button.querySelector("svg") !== null,
    );
    check(
      "while generating the placeholder reads Add a follow-up",
      editor.querySelector("p")?.getAttribute("data-placeholder") === "Add a follow-up",
    );
    button.click();
    check(
      "a click on Stop emits stop, not send or mic-press",
      sink.events.length === 1 && sink.events[0].type === "stop",
    );
    pressEnter(editor);
    check("Enter while generating emits nothing: Stop is the button's alone", sink.events.length === 1);
    input.setText("typed");
    check("text in the box does not turn Stop into send", button.getAttribute("data-state") === "stop");
    input.clear();
    input.update({ action: "send" });
    check(
      "the turn ending brings the mic back over an empty box, with the owner's placeholder",
      button.getAttribute("data-state") === "mic" &&
        editor.querySelector("p")?.getAttribute("data-placeholder") === "Plan, Build",
    );
    input.dispose();
  }

  // --- The mode picks the fill ----------------------------------------------------------

  {
    const input = new ChatBox({ mode: "plan" });
    const button = actionButton(input);
    check("the mode prop mirrors onto the button", button.getAttribute("data-mode") === "plan" && input.props.mode === "plan");
    for (const mode of ["ask", "debug", "multitask", "agent"]) {
      input.update({ mode });
      check(`the mode ${mode} mirrors onto the button`, button.getAttribute("data-mode") === mode);
    }
    input.dispose();
  }

  // --- Held and Ctrl Enter ---------------------------------------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>hello</p>" }, sink);
    const editor = editorElement(input);
    pressEnter(editor, { repeat: true });
    check("an auto-repeating Enter does not send", sink.sends() === 0);
    check("an auto-repeating Enter does not split the paragraph either", input.getText() === "hello");
    pressEnter(editor);
    check("the first Enter of a hold sends", sink.sends() === 1);
    pressEnter(editor, { ctrlKey: true });
    check("Ctrl+Enter sends", sink.sends() === 2);
    pressEnter(editor, { metaKey: true });
    check("Cmd+Enter sends", sink.sends() === 3);
    input.dispose();
  }

  {
    // The non-editable path (a dictation take holds the box) follows the same rule.
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>take</p>" }, sink);
    document.body.appendChild(input.element);
    input.setReadOnly(true);
    const frame = frameElement(input);
    frame.dispatchEvent(
      new dom.window.KeyboardEvent("keydown", { key: "Enter", repeat: true, bubbles: true, cancelable: true }),
    );
    check("a held Enter over a read-only box sends nothing", sink.sends() === 0);
    frame.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    check("the first Enter over a read-only box still sends", sink.sends() === 1);
    input.setReadOnly(false);
    input.dispose();
    input.element.remove();
  }

  // --- The button's state follows the box and the mic ----------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({}, sink);
    const mic = micButton(input);
    mic.click();
    check(
      "a click on the mic emits mic-press",
      sink.events.length === 1 && sink.events[0].type === "mic-press",
    );
    input.update({ mic: "recording" });
    check(
      "recording presses the mic, paints the recording class, and swaps the title",
      mic.getAttribute("data-mic") === "recording" &&
        mic.getAttribute("aria-pressed") === "true" &&
        mic.classList.contains("ws-stt-mic--recording") &&
        mic.title === "Stop recording" &&
        input.props.mic === "recording",
    );
    mic.click();
    check("a click while recording still emits mic-press", sink.events.length === 2);
    // Dictated words land in the box while the take records; the button must
    // stay the mic so a second press can end the take.
    input.setText("dictated words");
    check(
      "a recording mic stays the mic whatever the box holds",
      mic.getAttribute("data-state") === "mic" && mic.classList.contains("ws-stt-mic--recording"),
    );
    mic.click();
    check("a press on a recording mic over a filled box still emits mic-press", sink.events.length === 3);
    input.update({ mic: "blocked" });
    check(
      "blocked ends the recording look, and with text in the box the button is the send arrow",
      mic.getAttribute("data-mic") === "blocked" &&
        !mic.classList.contains("ws-stt-mic--recording") &&
        mic.getAttribute("data-state") === "send" &&
        !mic.hasAttribute("aria-pressed") &&
        mic.disabled === false,
    );
    input.clear();
    check(
      "a blocked mic over an empty box reads as an idle one",
      mic.getAttribute("data-state") === "mic" &&
        mic.getAttribute("aria-pressed") === "false" &&
        mic.title === "Voice Input (Ctrl+Shift+Space)" &&
        mic.disabled === false,
    );
    mic.click();
    check("a click while blocked still emits mic-press so the owning part can name the blocker", sink.events.length === 4);
    input.update({ mic: "idle" });
    check(
      "idle restores the default rendering",
      mic.getAttribute("data-mic") === "idle" &&
        mic.getAttribute("aria-pressed") === "false" &&
        mic.title === "Voice Input (Ctrl+Shift+Space)",
    );
    input.update({ action: "idle" });
    check("an idle agent leaves the mic enabled over an empty box", mic.disabled === false && mic.getAttribute("data-state") === "mic");
    input.setText("x");
    check("an idle agent dims the send arrow once there is text", mic.getAttribute("data-state") === "send" && mic.disabled === true);
    input.dispose();
  }

  // --- Typing, restoring, and clearing move the button too ---------------------------------

  {
    const input = new ChatBox();
    const button = actionButton(input);
    const editor = editorElement(input).editor;
    editor.commands.insertContent("typed");
    check("typing turns the mic into the send arrow", button.getAttribute("data-state") === "send");
    editor.commands.clearContent();
    check("deleting everything brings the mic back", button.getAttribute("data-state") === "mic");
    input.insertMention({ id: "a.ts", label: "a.ts", data: null });
    check("a lone mention pill counts as content", button.getAttribute("data-state") === "send");
    input.restore({ v: 1, doc: { type: "doc", content: [{ type: "paragraph" }] }, attachments: [] });
    check("restoring an empty draft brings the mic back", button.getAttribute("data-state") === "mic");
    input.dispose();
  }

  // --- The controls slot ------------------------------------------------------------

  {
    const controls = document.createElement("div");
    controls.className = "owner-toolbar";
    const existing = document.createElement("span");
    controls.appendChild(existing);
    const input = new ChatBox({ controls });
    const frame = frameElement(input);
    const button = actionButton(input);
    check(
      "the controls element sits on the bar after the frame",
      controls.parentElement === input.element && frame.nextElementSibling === controls,
    );
    check(
      "with controls the action button is the last child, after the owning part's own",
      controls.children.length === 2 &&
        controls.children[0] === existing &&
        controls.children[1] === button &&
        input.element.querySelector(":scope > .ws-agent-session__action") === null,
    );
    input.dispose();
    check(
      "dispose removes the box's button from the controls element and leaves the owning part's",
      controls.children.length === 1 && controls.children[0] === existing,
    );
  }

  // --- update() with unchanged props touches no DOM ----------------------------------

  {
    const input = new ChatBox({ mic: "recording", action: "send-blocked", editable: false });
    document.body.appendChild(input.element);
    const observer = new dom.window.MutationObserver(() => {});
    observer.observe(input.element, {
      attributes: true,
      childList: true,
      characterData: true,
      subtree: true,
    });
    input.update({ mic: "recording", action: "send-blocked", editable: false });
    input.update({});
    check(
      "an update with unchanged values mutates nothing",
      observer.takeRecords().length === 0,
    );
    input.update({ mic: "idle" });
    const changed = observer.takeRecords();
    check(
      "an update with one changed value mutates only the mic button",
      changed.length > 0 && changed.every((record) => record.target === micButton(input)),
    );
    observer.disconnect();
    input.dispose();
    input.element.remove();
  }

  // --- send includes the pills present -----------------------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox(
      {
        content:
          '<p>see <span data-type="mentionNode" data-id="src/main.ts" data-label="main.ts" data-kind="file" data-payload=\'{"path":"src/main.ts"}\'></span> and <span data-type="mentionNode" data-id="README.md" data-label="README.md"></span></p>',
      },
      sink,
    );
    sendButton(input).click();
    const event = sink.events[0];
    check(
      "send lists one ChipRef per pill in document order with the stored subset",
      event?.type === "send" &&
        event.mentions.length === 2 &&
        event.mentions[0].id === "src/main.ts" &&
        event.mentions[0].label === "main.ts" &&
        event.mentions[0].kind === "file" &&
        JSON.stringify(event.mentions[0].data) === '{"path":"src/main.ts"}' &&
        event.mentions[0].description === undefined &&
        event.mentions[0].group === undefined &&
        event.mentions[1].id === "README.md" &&
        event.mentions[1].kind === undefined &&
        event.mentions[1].data === null,
    );
    check(
      "send's attachments are empty while the strip is empty",
      event?.attachments.length === 0,
    );
    check(
      "send's text renders each pill through the editor's text serializer",
      typeof event?.text === "string" && event.text.startsWith("see "),
    );
    input.dispose();
  }

  // --- insertMention places a pill at the cursor ----------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({ content: "<p>see</p>" }, sink);
    // ProseMirror positions: paragraph opens at 0, "see" spans 1..4.
    input.setSelection(4, 4);
    const chip = { id: "src/main.ts", label: "main.ts", kind: "file", data: { path: "src/main.ts" } };
    input.insertMention(chip);
    const pill = editorElement(input).querySelector(".ws-mention-chip");
    check(
      "insertMention renders one pill through the NodeView at the cursor",
      pill !== null &&
        pill.getAttribute("data-id") === "src/main.ts" &&
        pill.getAttribute("data-kind") === "file" &&
        pill.querySelector(".ws-mention-chip__label")?.textContent === "main.ts",
    );
    const inline = input.serialize().doc.content?.[0]?.content ?? [];
    check(
      "the pill follows the text and is followed by exactly one space",
      inline.length === 3 &&
        inline[0]?.type === "text" &&
        inline[0].text === "see" &&
        inline[1]?.type === "mentionNode" &&
        inline[2]?.type === "text" &&
        inline[2].text === " ",
    );
    const after = input.insertionContext().range;
    check(
      "the cursor lands after the trailing space (text + node + space)",
      after.start === 6 && after.end === 6,
    );
    sendButton(input).click();
    const event = sink.events[0];
    check(
      "a send after insertMention lists the inserted chip with its payload intact",
      event?.type === "send" &&
        event.mentions.length === 1 &&
        event.mentions[0].id === "src/main.ts" &&
        event.mentions[0].kind === "file" &&
        JSON.stringify(event.mentions[0].data) === JSON.stringify(chip.data),
    );
    input.setSelection(1, 1);
    input.insertMention({ id: "README.md", label: "README.md", data: null });
    const front = input.serialize().doc.content?.[0]?.content ?? [];
    check(
      "insertMention at the paragraph start places the pill before the text",
      front[0]?.type === "mentionNode" &&
        front[0].attrs?.id === "README.md" &&
        front[1]?.type === "text" &&
        front[1].text === " see",
    );
    input.dispose();
  }

  // --- serialize / restore round-trip -------------------------------------------------

  {
    const source = new ChatBox({
      content:
        '<p>look at <span data-type="mentionNode" data-id="src/main.ts" data-label="main.ts" data-kind="file" data-payload=\'{"path":"src/main.ts","nested":[1,{"k":"v"}]}\'></span> first</p><p>then</p>',
    });
    const draft = source.serialize();
    check(
      "serialize returns v: 1, the ProseMirror JSON document, and empty attachments",
      draft.v === 1 &&
        draft.doc.type === "doc" &&
        Array.isArray(draft.attachments) &&
        draft.attachments.length === 0,
    );
    const attachment = { id: "img-1", label: "shot.png", kind: "image", data: { fileId: 7 } };
    const withStrip = { ...draft, attachments: [attachment] };

    const sink = recordingSink();
    const target = new ChatBox({}, sink);
    const frame = frameElement(target);
    target.restore(withStrip);
    check("restore reproduces the text", target.getText() === source.getText());
    const restored = target.serialize();
    check(
      "serialize after restore reproduces the document byte-for-byte",
      JSON.stringify(restored.doc) === JSON.stringify(draft.doc),
    );
    const pillNode = restored.doc.content?.[0]?.content?.find((node) => node.type === "mentionNode");
    check(
      "the pill's opaque payload survives the round-trip byte-for-byte",
      pillNode !== undefined &&
        JSON.stringify(pillNode.attrs?.data) === '{"path":"src/main.ts","nested":[1,{"k":"v"}]}',
    );
    check(
      "restore paints the pill in the editor",
      editorElement(target).querySelector(".ws-mention-chip[data-id='src/main.ts']") !== null,
    );
    const strip = frame.querySelector(".ws-prompt-input__attachments");
    check(
      "restore paints one chip per attachment into the strip",
      strip.childElementCount === 1 &&
        strip.firstElementChild?.classList.contains("ws-mention-chip") === true &&
        strip.firstElementChild?.getAttribute("data-kind") === "image" &&
        strip.querySelector(".ws-mention-chip__label")?.textContent === "shot.png",
    );
    check(
      "the restored strip pill omits the remove button",
      strip.querySelector(".ws-mention-chip__remove") === null,
    );
    check(
      "serialize after restore returns the attachments as a copy",
      JSON.stringify(restored.attachments) === JSON.stringify([attachment]) &&
        restored.attachments !== withStrip.attachments,
    );
    sendButton(target).click();
    check(
      "send after restore includes the restored pill and attachments",
      sink.events[0]?.type === "send" &&
        sink.events[0].mentions.length === 1 &&
        sink.events[0].mentions[0].id === "src/main.ts" &&
        JSON.stringify(sink.events[0].attachments) === JSON.stringify([attachment]),
    );
    target.restore({ v: 1, doc: { type: "doc", content: [] }, attachments: [] });
    check(
      "restoring an empty draft clears the text and the strip",
      target.getText() === "" && strip.childElementCount === 0,
    );
    source.dispose();
    target.dispose();
  }

  // --- restore rejects an unknown or missing version -----------------------------------

  {
    const input = new ChatBox({ content: "<p>keep me</p>" });
    const strip = frameElement(input).querySelector(".ws-prompt-input__attachments");
    const before = JSON.stringify(input.serialize());
    const foreign = { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "replaced" }] }] };
    const attachment = { id: "img-1", label: "shot.png", kind: "image", data: null };
    input.restore({ v: 2, doc: foreign, attachments: [attachment] });
    check(
      "restore with an unknown version leaves the text, the strip, and the serialized form unchanged",
      input.getText() === "keep me" &&
        strip.childElementCount === 0 &&
        JSON.stringify(input.serialize()) === before,
    );
    input.restore({ doc: foreign, attachments: [attachment] });
    check(
      "restore with a missing version leaves the box unchanged",
      input.getText() === "keep me" &&
        strip.childElementCount === 0 &&
        JSON.stringify(input.serialize()) === before,
    );
    input.restore({ v: "1", doc: foreign, attachments: [attachment] });
    check(
      "restore with a string version is not coerced to 1",
      input.getText() === "keep me" && strip.childElementCount === 0,
    );
    input.dispose();
  }

  // --- The mention source seam -----------------------------------------------------------

  {
    const all = await stubMentionSource("", new AbortController().signal);
    check(
      "the default stub source lists its three canned entries as chips",
      all.length === 3 &&
        all[0].label === "README.md" &&
        all[1].label === "src/main.ts" &&
        all[2].label === "Cargo.toml" &&
        all.every((chip) => chip.id === chip.label && chip.kind === "file" && chip.data === null),
    );
    const narrowed = await stubMentionSource("RE", new AbortController().signal);
    check(
      "the stub source filters by case-insensitive substring on the label",
      narrowed.length === 1 &&
        narrowed[0].label === "README.md" &&
        (await stubMentionSource("re", new AbortController().signal)).length === 1 &&
        (await stubMentionSource("zzz", new AbortController().signal)).length === 0,
    );
  }

  {
    const { input, editor } = mountedBox();
    await typeText(editor, "@");
    check(
      "with no mentionSource, typing @ lists the stub entries",
      popupLabels().join(",") === "README.md,src/main.ts,Cargo.toml",
    );
    input.dispose();
    input.element.remove();
  }

  {
    const seen = [];
    const mentionSource = async (query, signal) => {
      seen.push({ query, aborted: signal instanceof AbortSignal ? signal.aborted : null });
      return [
        { id: "docs/alpha.md", label: "alpha.md", kind: "file", description: "docs", data: { p: 1 } },
        { id: "docs/beta.md", label: "beta.md", kind: "file", description: "docs", data: { p: 2 } },
      ].filter((chip) => chip.label.includes(query));
    };
    const { input, editor } = mountedBox({ mentionSource });
    await typeText(editor, "@");
    check(
      "an injected mentionSource replaces the stub and the popup lists its items",
      popupLabels().join(",") === "alpha.md,beta.md" && popup()?.hidden === false,
    );
    check(
      "the source is called with the query and a live AbortSignal",
      seen.length >= 1 && seen[0].query === "" && seen[0].aborted === false,
    );
    await typeText(editor, "bet");
    check(
      "a narrowed query reaches the source and filters the popup",
      seen.at(-1)?.query === "bet" && popupLabels().join(",") === "beta.md",
    );
    input.dispose();
    input.element.remove();
    check("disposing the box mid-session removes the popup", popup() === null);
  }

  {
    const mentionSource = deferredSource();
    const { input, editor } = mountedBox({ mentionSource });
    await typeText(editor, "@");
    check(
      "the pending source has been asked for the empty query",
      mentionSource.calls.length === 1 && mentionSource.calls[0].query === "",
    );
    await typeText(editor, "x");
    check(
      "a newer keystroke fires the older query's AbortSignal",
      mentionSource.calls[0].signal.aborted === true &&
        mentionSource.calls.length === 2 &&
        mentionSource.calls[1].query === "x" &&
        mentionSource.calls[1].signal.aborted === false,
    );
    mentionSource.calls[1].resolve([{ id: "x1", label: "xylophone.ts", data: null }]);
    await settle();
    check(
      "the newer query's results fill the popup",
      popupLabels().join(",") === "xylophone.ts",
    );
    mentionSource.calls[0].resolve([{ id: "old", label: "stale.ts", data: null }]);
    await settle();
    check(
      "an older query resolving after a newer one does not overwrite the newer results",
      popupLabels().join(",") === "xylophone.ts",
    );
    input.dispose();
    input.element.remove();
  }

  {
    const { input, editor } = mountedBox();
    await typeText(editor, "/");
    check(
      "a typed / is plain text with no popup while commandSource is the default",
      popup() === null && input.getText() === "/",
    );
    await typeText(editor, "help");
    check("text after / stays text", popup() === null && input.getText() === "/help");
    input.dispose();
    input.element.remove();
  }

  // --- The static renderer (chat-box-view.ts) ----------------------------------------

  {
    const attachment = { id: "img-1", label: "shot.png", kind: "image", data: { fileId: 7 } };
    const pill = { id: "src/main.ts", label: "main.ts", kind: "file", data: { path: "src/main.ts" } };
    const draft = {
      v: 1,
      doc: {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [
              { type: "text", text: "look at " },
              { type: "mentionNode", attrs: { ...pill, mentionSuggestionChar: "@" } },
              { type: "text", text: " first" },
              { type: "hardBreak" },
              { type: "text", text: "then" },
            ],
          },
          { type: "paragraph" },
          { type: "paragraph", content: [{ type: "text", text: "done" }] },
        ],
      },
      attachments: [attachment],
    };
    const fragment = renderDraft(draft);
    const root = fragment.firstElementChild;
    check(
      "renderDraft returns a fragment holding one ws-draft-view root",
      fragment.childElementCount === 1 && root?.classList.contains("ws-draft-view") === true,
    );
    const strip = root?.firstElementChild;
    check(
      "the attachments strip comes first and holds one pill per attachment",
      strip?.classList.contains("ws-draft-view__strip") === true &&
        strip.querySelectorAll(".ws-mention-chip").length === 1 &&
        strip.querySelector(".ws-mention-chip")?.getAttribute("data-kind") === "image" &&
        strip.querySelector(".ws-mention-chip__label")?.textContent === "shot.png",
    );
    const paragraphs = [...(root?.querySelectorAll(".ws-draft-view__paragraph") ?? [])];
    check(
      "one paragraph element per paragraph node, in order, after the strip",
      paragraphs.length === 3 &&
        paragraphs[0] === strip?.nextElementSibling &&
        paragraphs[2] === root?.lastElementChild,
    );
    const first = paragraphs[0];
    check(
      "text, the inline pill, and the hard break land in document order",
      first !== undefined &&
        first.childNodes[0]?.nodeType === dom.window.Node.TEXT_NODE &&
        first.childNodes[0]?.textContent === "look at " &&
        first.childNodes[1]?.classList?.contains("ws-mention-chip") === true &&
        first.childNodes[1]?.getAttribute("data-kind") === "file" &&
        first.childNodes[1]?.querySelector(".ws-mention-chip__label")?.textContent === "main.ts" &&
        first.childNodes[2]?.textContent === " first" &&
        first.childNodes[3]?.tagName === "BR" &&
        first.childNodes[4]?.textContent === "then",
    );
    check(
      "an empty paragraph renders as an empty paragraph element",
      paragraphs[1]?.childNodes.length === 0 && paragraphs[2]?.textContent === "done",
    );
    check(
      "the read-only rendering omits the remove buttons and the editor",
      root?.querySelector(".ws-mention-chip__remove") === null &&
        root?.querySelector(".ProseMirror") === null &&
        root?.querySelector('[contenteditable="true"]') === null,
    );
    const empty = renderDraft({ v: 1, doc: { type: "doc", content: [] }, attachments: [] });
    check(
      "an empty draft renders a root with an empty strip and no paragraphs",
      empty.firstElementChild?.querySelector(".ws-draft-view__strip")?.childElementCount === 0 &&
        empty.firstElementChild?.querySelectorAll(".ws-draft-view__paragraph").length === 0,
    );
  }

  // --- The error popup -----------------------------------------------------------------------

  {
    const sink = recordingSink();
    const input = new ChatBox({}, sink);
    const popup = () => input.element.querySelector(".ws-chat-error");
    const retry = () => popup()?.querySelector(".ws-chat-error__retry");
    check("a box with no error has no popup", popup() === null && input.props.error === null);

    input.update({ error: { message: "The socket closed." } });
    check(
      "an error opens the popup on the card, an alert",
      popup() !== null && popup().parentElement === input.element && popup().getAttribute("role") === "alert",
    );
    check(
      "the title defaults to Connection Error and the message sits below it",
      popup().querySelector(".ws-chat-error__title")?.textContent === "Connection Error" &&
        popup().querySelector(".ws-chat-error__message")?.textContent === "The socket closed.",
    );
    check(
      "the popup carries the warning glyph",
      popup().querySelector(".ws-chat-error__icon svg") !== null,
    );
    check("without tryAgain there is no Try again button shown", retry()?.hidden === true);

    input.update({ error: { title: "Connection failed", message: "Check your network.", tryAgain: "disabled" } });
    check(
      "a given title replaces the default, and the popup is reused in place",
      popup().querySelector(".ws-chat-error__title")?.textContent === "Connection failed" &&
        input.element.querySelectorAll(".ws-chat-error").length === 1,
    );
    check(
      "Try again shows disabled until the owner enables it",
      retry()?.hidden === false && retry().disabled === true && retry().textContent === "Try again",
    );
    retry().click();
    check("a disabled Try again reports nothing", sink.events.every((event) => event.type !== "retry"));

    input.update({ error: { title: "Connection failed", message: "Check your network.", tryAgain: "enabled" } });
    check("the owner enables Try again", retry().disabled === false);
    retry().click();
    check(
      "pressing Try again emits retry",
      sink.events.filter((event) => event.type === "retry").length === 1,
    );
    check(
      "Try again is the secondary small button",
      retry().classList.contains("button-secondary") && retry().classList.contains("button-sm"),
    );

    const same = popup();
    input.update({ error: { title: "Connection failed", message: "Check your network.", tryAgain: "enabled" } });
    check("an unchanged error touches nothing", popup() === same);

    input.update({ error: { message: "<b>markup</b>" } });
    check(
      "the message is text, never markup",
      popup().querySelector("b") === null &&
        popup().querySelector(".ws-chat-error__message")?.textContent === "<b>markup</b>",
    );

    input.update({ error: null });
    check("a null error removes the popup", popup() === null && input.props.error === null);
    input.dispose();
  }

  {
    const input = new ChatBox({ error: { message: "from construction" } });
    check(
      "an error given at construction opens the popup",
      input.element.querySelector(".ws-chat-error__message")?.textContent === "from construction",
    );
    input.dispose();
  }
});

// --- The stylesheet: the card, the editor, and the action button ---------------------------

{
  const css = (
    await readFile(path.join(testDir, "..", "src", "parts", "chatbox", "chat-box.css"), "utf8")
  ).replace(/\/\*[\s\S]*?\*\//g, "");
  const lookTokens = await readFile(path.join(testDir, "..", "..", "look", "tokens.css"), "utf8");
  const componentCss = await readFile(path.join(testDir, "..", "src", "tokens", "component.css"), "utf8");
  const sessionCss = await readFile(path.join(testDir, "..", "src", "parts", "agent", "agent-session.css"), "utf8");
  const rule = (selector) => {
    const match = new RegExp(`${selector.replace(/[.[\]"=():>~+*]/g, "\\$&")}\\s*\\{([^}]*)\\}`).exec(css);
    return match === null ? "" : match[1].replace(/\s+/g, " ");
  };
  const token = (name) => new RegExp(`${name}:\\s*([^;]+);`).exec(lookTokens)?.[1].trim();

  const card = rule(".ws-agent-session__bar");
  check(
    "the card's fill is the input color at 90%, about #202020",
    token("--prompt-input-bg") === "color-mix(in srgb, var(--bg-input) 90%, transparent)" &&
      /background:\s*var\(--prompt-input-bg\)/.test(card),
  );
  check(
    "the card keeps its 12px radius and 8/10/6 padding",
    token("--agent-card-radius") === "12px" &&
      /border-radius:\s*var\(--agent-card-radius\)/.test(card) &&
      /padding:\s*var\(--space-2\) var\(--space-2-5\) var\(--space-1-5\)/.test(card) &&
      token("--space-2") === "8px" &&
      token("--space-2-5") === "10px" &&
      token("--space-1-5") === "6px",
  );
  check(
    "the card's border and shadow ease over 100ms",
    /transition:\s*border-color var\(--duration-fast\) ease-in-out, box-shadow var\(--duration-fast\) ease-in-out/.test(card) &&
      token("--duration-fast") === "100ms",
  );
  check(
    "the card sits 10px from each side",
    token("--agent-outer-padding") === "10px" && /padding-inline:\s*var\(--agent-outer-padding\)/.test(sessionCss),
  );
  check(
    "the toolbar sits 9px under the editor",
    /gap:\s*var\(--ws-composer-toolbar-gap\)/.test(card) && /--ws-composer-toolbar-gap:\s*var\(--ws-size-9\)/.test(componentCss),
  );
  check(
    "the card is the composer container the toolbar's breakpoints answer to",
    /container:\s*ws-composer \/ inline-size/.test(card),
  );

  const editor = rule(".ws-prompt-input__editor");
  check(
    "the editor is 14px/22px with no letter-spacing",
    /font-size:\s*var\(--font-size-lg\)/.test(editor) &&
      /line-height:\s*var\(--line-height-lg\)/.test(editor) &&
      /letter-spacing:\s*normal/.test(editor) &&
      token("--font-size-lg") === "14px" &&
      token("--line-height-lg") === "22px",
  );
  check(
    "the editor grows from 22px to 240px",
    token("--prompt-input-min-height") === "22px" && token("--prompt-input-max-height") === "240px",
  );

  check(
    "the action button is a round control with a #141414 glyph on the Agent fill",
    /border-radius:\s*var\(--radius-full\)/.test(rule(".ws-agent-session__action")) &&
      /color:\s*var\(--cursor-chrome\)/.test(rule(".ws-agent-session__action")) &&
      /background:\s*var\(--cursor-base\)/.test(rule(".ws-agent-session__action")) &&
      token("--cursor-chrome") === "#141414",
  );
  check(
    "the mic is a 66% fill under a #181818 glyph",
    /color:\s*var\(--cursor-editor\)/.test(rule('.ws-agent-session__action[data-state="mic"]')) &&
      /background:\s*var\(--cursor-icon-secondary\)/.test(rule('.ws-agent-session__action[data-state="mic"]')) &&
      /--cursor-icon-secondary:\s*color-mix\(in srgb, var\(--cursor-base\) 66%, transparent\)/.test(lookTokens),
  );
  check(
    "Plan fills yellow and Ask green, for send and Stop alike",
    /\[data-state="send"\]\[data-mode="plan"\],\s*\.ws-agent-session__action\[data-state="stop"\]\[data-mode="plan"\]\s*\{[^}]*background:\s*var\(--cursor-yellow\)/.test(css) &&
      /\[data-state="send"\]\[data-mode="ask"\],\s*\.ws-agent-session__action\[data-state="stop"\]\[data-mode="ask"\]\s*\{[^}]*background:\s*var\(--cursor-green\)/.test(css) &&
      token("--cursor-yellow") === "#F1B467" &&
      token("--cursor-green") === "#3FA266",
  );
  check(
    "Debug and Multitask fill the base color at 80%",
    /\[data-mode="debug"\][^{]*\[data-mode="multitask"\][^{]*\{[^}]*color-mix\(in srgb, var\(--cursor-base\) 80%, transparent\)/.test(css),
  );
  check(
    "a disabled button keeps its fill at opacity 0.3",
    /opacity:\s*0\.3/.test(rule(".ws-agent-session__action:disabled")) &&
      !/background/.test(rule(".ws-agent-session__action:disabled")),
  );
  check(
    "the recording look survives on the one button",
    /ws-agent-session__action\.ws-stt-mic--recording/.test(css) &&
      !/\.ws-agent-session__(mic|send)\b/.test(css),
  );
}

if (failures.length > 0) {
  console.error(`chat-box: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("chat-box: all assertions passed");
process.exit(0);
