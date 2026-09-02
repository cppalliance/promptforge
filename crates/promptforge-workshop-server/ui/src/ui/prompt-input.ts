// The prompt input: a Tiptap/ProseMirror editor framed as the chat box.
// The schema is deliberately minimal - paragraphs, text, and hard breaks
// - so what the operator types is plain text with newlines; richer nodes
// (mention chips) join as extensions on top of this base. Enter submits
// through the onSubmit callback; an Enter that commits an IME
// composition never submits; Shift+Enter inserts a hard break. The box
// grows with its content: every edit re-measures scrollHeight and clamps
// it between the skin's min/max height tokens.

import "./prompt-input.css";

import { Editor } from "@tiptap/core";
import { Placeholder } from "@tiptap/extension-placeholder";
import { StarterKit } from "@tiptap/starter-kit";
import { Disposable, toDisposable } from "../base/lifecycle";
import { MentionChip, MentionSuggestionPluginKey } from "./workshop/mention-chip";

// The fallbacks mirror the token defaults in style.css; they apply when
// the skin is absent (tests) or the token is deleted.
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
  const parsed = Number.parseFloat(getComputedStyle(element).getPropertyValue(token));
  return Number.isFinite(parsed) ? parsed : fallback;
}

/** Construction options for {@link PromptInput}. */
export interface PromptInputOptions {
  /** Placeholder text while the editor is empty. */
  readonly placeholder?: string;
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
 */
export class PromptInput extends Disposable {
  /** The framed container; append it where the input belongs. */
  readonly element: HTMLDivElement;

  private readonly editor: Editor;

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
          placeholder: options.placeholder ?? "Message the agent",
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
          // An Enter that commits an IME composition is not a send:
          // without the isComposing guard the box would submit
          // half-composed text.
          if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
            // An open mention typeahead owns Enter - it inserts the
            // highlighted item. editorProps handlers run before the
            // suggestion state plugin's, so without this check the
            // submit would fire instead of the selection.
            if (MentionSuggestionPluginKey.getState(view.state)?.active === true) {
              return false;
            }
            options.onSubmit?.();
            return true;
          }
          return false;
        },
      },
      onUpdate: () => {
        this.syncHeight();
      },
    });
    this._register(
      toDisposable(() => {
        this.editor.destroy();
      }),
    );
    this.syncHeight();
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
   * Gates editing. ProseMirror has no `disabled`; a non-editable editor
   * is the equivalent, and the pending-input wait maps onto it.
   */
  setEditable(editable: boolean): void {
    this.editor.setEditable(editable);
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
