// The prompt input: a Tiptap/ProseMirror editor framed as the chat box.
// The schema is deliberately minimal - paragraphs, text, and hard breaks
// - so what the operator types is plain text with newlines; richer nodes
// (mention chips) join as extensions on top of this base. Enter submits
// through the onSubmit callback; an Enter that commits an IME
// composition never submits; Shift+Enter inserts a hard break. The box
// grows with its content: every edit re-measures scrollHeight and clamps
// it between the skin's min/max height tokens.

import "./prompt-input.css";

import { Editor, type JSONContent } from "@tiptap/core";
import { Placeholder } from "@tiptap/extension-placeholder";
import { StarterKit } from "@tiptap/starter-kit";
import { Disposable, toDisposable } from "../base/lifecycle";
import type { SttInputTarget } from "./stt";
import { MentionChip, MentionSuggestionPluginKey } from "./workshop/mention-chip";

// The fallbacks mirror the token defaults in shared-ui/tokens.css; they
// apply when the skin is absent (tests) or the token is deleted.
const DEFAULT_MIN_HEIGHT_PX = 36;
const DEFAULT_MAX_HEIGHT_PX = 200;

/**
 * Clamps a measured content height into the input's height band.
 * Exported so tests can pin the band logic directly: jsdom reports a
 * scrollHeight of 0, so the measurement itself cannot be exercised there.
 */
export function clampPromptInputHeight(
  contentHeight: number,
  minHeight: number,
  maxHeight: number,
): number {
  return Math.min(Math.max(contentHeight, minHeight), maxHeight);
}

/** Reads a pixel-valued skin token, falling back when unset or unparseable. */
function readPixelToken(element: HTMLElement, token: string, fallback: number): number {
  // Read at the document root: the tokens are global (:root), and
  // reading a custom property off a deep element hits jsdom's uncached,
  // ancestor-recursing custom-property resolution - exponential in DOM
  // depth (https://github.com/jsdom/jsdom/issues/3234).
  const parsed = Number.parseFloat(
    getComputedStyle(element.ownerDocument.documentElement).getPropertyValue(token),
  );
  return Number.isFinite(parsed) ? parsed : fallback;
}

/** Construction options for {@link PromptInput}. */
export interface PromptInputOptions {
  /**
   * Placeholder text while the editor is empty. A function is
   * re-evaluated on every state update, so a host can name the current
   * gate (a pending wait opening and closing) without rebuilding the
   * editor.
   */
  readonly placeholder?: string | (() => string);
  /** Accessible label on the editable region. */
  readonly ariaLabel?: string;
  /** Initial content, parsed as HTML (`<p>` per paragraph). */
  readonly content?: string;
  /** Called on a submitting Enter - never on Shift+Enter or mid-composition. */
  readonly onSubmit?: () => void;
}

/**
 * The framed rich-text prompt box. Disposable: dispose() destroys the
 * editor, which empties and unwires the ProseMirror DOM.
 *
 * Implements {@link SttInputTarget}: dictation splices the transcript in
 * through getSelection/replaceRange and holds the box with setReadOnly.
 * The target's offsets are ProseMirror positions.
 */
export class PromptInput extends Disposable implements SttInputTarget {
  /** The framed container; append it where the input belongs. */
  readonly element: HTMLDivElement;

  private readonly editor: Editor;

  // Two locks, one property: the pending-wait gate (setEditable) and a
  // dictation take (setReadOnly) both map onto contenteditable, because
  // ProseMirror has no separate readOnly. Each side keeps its own flag
  // so one lock lifting never reopens the other - a take that outlives
  // its wait must not leave the box editable against the dead wait.
  private gateEditable = true;
  private takeReadOnly = false;

  constructor(options: PromptInputOptions = {}) {
    super();
    this.element = document.createElement("div");
    this.element.className = "prompt-input";

    this.editor = new Editor({
      element: this.element,
      extensions: [
        // Plain-text schema: everything in StarterKit is off except the
        // document scaffolding (document, paragraph, text, gapcursor)
        // and hardBreak, whose Shift-Enter binding supplies newlines.
        StarterKit.configure({
          blockquote: false,
          bold: false,
          bulletList: false,
          code: false,
          codeBlock: false,
          dropcursor: false,
          heading: false,
          horizontalRule: false,
          italic: false,
          link: false,
          listItem: false,
          listKeymap: false,
          orderedList: false,
          strike: false,
          trailingNode: false,
          underline: false,
          undoRedo: false,
        }),
        Placeholder.configure({
          placeholder: options.placeholder ?? "Plan, Build, / for skills, @ for context",
          // The gated (non-editable) box still carries its placeholder,
          // same as a disabled textarea: the gate's "the agent is
          // working" message IS the non-editable state.
          showOnlyWhenEditable: false,
        }),
        // Inline mention pills (@-referenced files) with the typeahead
        // popup wired into the extension's suggestion seam.
        MentionChip,
      ],
      content: options.content ?? "",
      editorProps: {
        attributes: {
          class: "prompt-input__editor",
          role: "textbox",
          "aria-label": options.ariaLabel ?? "Message",
          "aria-multiline": "true",
        },
        handleKeyDown: (view, event) => {
          if (event.key !== "Enter" || event.shiftKey) {
            return false;
          }
          // An Enter that commits an IME composition is not a send:
          // without the isComposing guard the box would submit
          // half-composed text. Claimed, not passed on: the keymap would
          // otherwise split the paragraph under the composition.
          if (event.isComposing) {
            return true;
          }
          // An open mention typeahead owns Enter - it inserts the
          // highlighted item. editorProps handlers run before the
          // suggestion state plugin's, so without this check the
          // submit would fire instead of the selection.
          if (MentionSuggestionPluginKey.getState(view.state)?.active === true) {
            return false;
          }
          options.onSubmit?.();
          return true;
        },
      },
      onUpdate: () => {
        this.syncHeight();
      },
    });
    // prosemirror-view drops keydown events for a non-editable editor
    // before any handleKeyDown prop runs (its editHandlers gate), so the
    // submit above never fires while a dictation take holds the box
    // read-only - yet an Enter there is still a send, carrying what the
    // box shows. Listen at the frame for exactly that case; the editable
    // case belongs to the editorProps handler.
    this.element.addEventListener("keydown", (event) => {
      if (this.editor.isEditable) {
        return;
      }
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        options.onSubmit?.();
      }
    });
    this._register(
      toDisposable(() => {
        this.editor.destroy();
      }),
    );
    const initialMeasure = window.requestAnimationFrame(() => this.syncHeight());
    this._register(toDisposable(() => window.cancelAnimationFrame(initialMeasure)));
  }

  /** The prompt as plain text: paragraphs and hard breaks as single newlines. */
  getText(): string {
    return this.editor.getText({ blockSeparator: "\n" });
  }

  /** Empties the editor; the update hook re-clamps the height. */
  clear(): void {
    this.editor.commands.clearContent();
  }

  /**
   * Replaces the content with plain text (one paragraph per newline) and
   * leaves the cursor at the end. Built as JSON, never HTML-parsed, so
   * the text lands verbatim.
   */
  setText(text: string): void {
    const content: JSONContent = {
      type: "doc",
      content: text.split("\n").map((line) => ({
        type: "paragraph",
        content: line === "" ? undefined : [{ type: "text", text: line }],
      })),
    };
    this.editor.commands.setContent(content);
    this.editor.commands.setTextSelection(this.editor.state.doc.content.size - 1);
  }

  /** The selection as ProseMirror positions - the SttInputTarget coordinate space. */
  getSelection(): { start: number; end: number } {
    const { from, to } = this.editor.state.selection;
    return { start: from, end: to };
  }

  /** Places the cursor or selection at ProseMirror positions. */
  setSelection(from: number, to: number): void {
    this.editor.commands.setTextSelection({ from, to });
  }

  /**
   * Replaces [from, to] with plain text and leaves the cursor after the
   * inserted text. Newlines insert hard breaks, so the inserted text
   * occupies exactly text.length positions.
   */
  replaceRange(from: number, to: number, text: string): void {
    if (text === "") {
      this.editor.chain().deleteRange({ from, to }).setTextSelection(from).run();
      return;
    }
    const content: JSONContent[] = [];
    const lines = text.split("\n");
    for (let index = 0; index < lines.length; index++) {
      if (index > 0) {
        content.push({ type: "hardBreak" });
      }
      const line = lines[index];
      if (line !== undefined && line !== "") {
        content.push({ type: "text", text: line });
      }
    }
    this.editor
      .chain()
      .insertContentAt({ from, to }, content)
      .setTextSelection(from + text.length)
      .run();
  }

  /**
   * The dictation take's lock: non-editable plus the recording ring on
   * the frame (stt.css's `.stt-input--recording`). Composes with the
   * gate through the two flag fields.
   */
  setReadOnly(readOnly: boolean): void {
    this.takeReadOnly = readOnly;
    this.applyEditable();
    this.element.classList.toggle("stt-input--recording", readOnly);
  }

  /** Focuses the editor; a landed dictation final calls it. */
  focus(): void {
    this.editor.commands.focus();
  }

  /**
   * Gates editing. ProseMirror has no `disabled`; a non-editable editor
   * is the equivalent, and the pending-input wait maps onto it.
   */
  setEditable(editable: boolean): void {
    this.gateEditable = editable;
    this.applyEditable();
  }

  private applyEditable(): void {
    this.editor.setEditable(this.gateEditable && !this.takeReadOnly);
  }

  /**
   * Re-measures the content and re-clamps the box height. Runs on every
   * edit; exposed so an outside layout change (panel resize, zoom) can
   * force a re-measure.
   */
  syncHeight(): void {
    const dom = this.editor.view.dom;
    // scrollHeight never drops below the client height, so the box must
    // be released to its natural height before measuring, or it could
    // never shrink.
    dom.style.height = "auto";
    const min = readPixelToken(dom, "--prompt-input-min-height", DEFAULT_MIN_HEIGHT_PX);
    const max = readPixelToken(dom, "--prompt-input-max-height", DEFAULT_MAX_HEIGHT_PX);
    dom.style.height = `${clampPromptInputHeight(dom.scrollHeight, min, max)}px`;
  }
}
