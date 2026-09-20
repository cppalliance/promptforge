// The chat box: a Tiptap/ProseMirror editor framed on a bar with its mic
// and send buttons. The schema is deliberately minimal - paragraphs,
// text, and hard breaks - so what the operator types is plain text with
// newlines; richer nodes (mention chips) join as extensions on top of
// this base. Enter emits `send`; an Enter that commits an IME composition
// never does; Shift+Enter inserts a hard break. The box grows with its
// content: every edit re-measures scrollHeight and clamps it between the
// skin's min/max height tokens.
//
// The component is isolated: props in (every one defaulted), events out
// through one sink, an imperative handle for the host and for dictation.
// It reaches no service registry - the text-control registrar is
// injected - and imports nothing from the host layers. Every prop-driven
// state is mirrored onto the DOM as a data attribute beside the classes
// the skin already relies on.

import "./chat-box.css";

import { Editor, type JSONContent } from "@tiptap/core";
import { Placeholder } from "@tiptap/extension-placeholder";
import { redoDepth, undoDepth } from "@tiptap/pm/history";
import { StarterKit } from "@tiptap/starter-kit";
import { Disposable, type IDisposable, toDisposable } from "../../base/lifecycle";
import { ICON_MIC, ICON_SEND } from "../shared/icons";
import { renderChip } from "./chip-view";
import { type ChipNodeAttrs, chipFromAttrs, MentionChip, MentionSuggestionPluginKey } from "./mention-chip";
import type {
  ChatBoxDynamicProps,
  ChatBoxEventSink,
  ChatBoxHandle,
  ChatBoxProps,
  ChipRef,
  SerializedDraft,
} from "./types";

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

/** The resolved dynamic props: what `update()` drives and `props` reads back. */
type ResolvedDynamicProps = {
  -readonly [K in keyof ChatBoxDynamicProps]-?: ChatBoxDynamicProps[K];
};

/** The mic button's title per state; blocked reads as idle because the press is what names the blocker. */
function micTitle(mic: ResolvedDynamicProps["mic"]): string {
  return mic === "recording" ? "Stop recording" : "Push to talk";
}

/**
 * The chat box: the bar (`ws-agent-session__bar`) holding the framed
 * editor, an optional host-owned controls element, and the box's own mic
 * and send buttons. Disposable: dispose() destroys the editor, releases
 * the text-control registration, and takes its buttons back out of the
 * controls element it was handed.
 *
 * The handle is a structural superset of dictation's input target:
 * dictation splices the transcript in through insertionContext and
 * replaceRange and holds the box with setReadOnly. Offsets are
 * ProseMirror positions.
 */
export class ChatBox extends Disposable implements ChatBoxHandle {
  /** The bar; append it where the composer belongs. */
  readonly element: HTMLDivElement;

  private readonly frame: HTMLDivElement;
  private readonly strip: HTMLDivElement;
  private readonly mic: HTMLButtonElement;
  private readonly send: HTMLButtonElement;
  private readonly editor: Editor;
  private readonly onEvent: ChatBoxEventSink;
  private readonly variant: NonNullable<ChatBoxProps["variant"]>;
  private readonly dynamic: ResolvedDynamicProps;
  private attachments: ChipRef[] = [];

  // Two locks, one property: the pending-wait gate (the editable prop)
  // and a dictation take (setReadOnly) both map onto contenteditable,
  // because ProseMirror has no separate readOnly. Each side keeps its own
  // flag so one lock lifting never reopens the other - a take that
  // outlives its wait must not leave the box editable against the dead
  // wait.
  private takeReadOnly = false;

  constructor(props: ChatBoxProps = {}, onEvent: ChatBoxEventSink = () => {}) {
    super();
    this.onEvent = onEvent;
    this.variant = props.variant ?? "expanded";
    this.dynamic = {
      editable: props.editable ?? true,
      action: props.action ?? "send",
      mic: props.mic ?? "idle",
    };

    this.element = document.createElement("div");
    this.element.className = "ws-agent-session__bar";
    this.element.dataset["variant"] = this.variant;

    this.frame = document.createElement("div");
    this.frame.className = "ws-prompt-input";
    // The strip goes in before the editor mounts, so the ProseMirror
    // content element lands after it: attachments above the text.
    this.strip = document.createElement("div");
    this.strip.className = "ws-prompt-input__attachments";
    this.frame.appendChild(this.strip);

    this.mic = document.createElement("button");
    this.mic.type = "button";
    this.mic.className = "ws-agent-session__mic ws-stt-mic";
    this.mic.setAttribute("aria-label", "Push to talk");
    // Static lucide strings, not data: the only markup this box writes.
    this.mic.innerHTML = ICON_MIC;
    this.mic.addEventListener("click", () => this.onEvent({ type: "mic-press" }));
    this.send = document.createElement("button");
    this.send.type = "button";
    this.send.className = "ws-agent-session__send";
    this.send.setAttribute("aria-label", "Send");
    this.send.innerHTML = ICON_SEND;
    this.send.addEventListener("click", () => this.emitAction());

    // Two bar shapes, one owner: with a controls element the buttons
    // trail the host's toolbar; without one they sit on the bar. The
    // buttons are the box's in both cases.
    if (props.controls !== undefined) {
      props.controls.append(this.mic, this.send);
      this.element.append(this.frame, props.controls);
      const controls = props.controls;
      this._register(
        toDisposable(() => {
          if (this.mic.parentElement === controls) {
            this.mic.remove();
          }
          if (this.send.parentElement === controls) {
            this.send.remove();
          }
        }),
      );
    } else {
      this.element.append(this.frame, this.mic, this.send);
    }

    this.editor = new Editor({
      element: this.frame,
      extensions: [
        // Plain-text schema: everything in StarterKit is off except the
        // document scaffolding (document, paragraph, text, gapcursor),
        // hardBreak, whose Shift-Enter binding supplies newlines, and
        // undoRedo, whose history plugin backs the text-control
        // adapter's undo/redo (its Mod-z keymap never fires in the app:
        // the keybinding dispatcher claims the chord in the capture
        // phase).
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
        }),
        Placeholder.configure({
          placeholder: props.placeholder ?? "",
          // The gated (non-editable) box still carries its placeholder,
          // same as a disabled textarea: the gate's "the agent is
          // working" message IS the non-editable state.
          showOnlyWhenEditable: false,
        }),
        // Inline mention pills (@-referenced files) with the typeahead
        // popup wired into the extension's suggestion seam.
        MentionChip,
      ],
      content: props.content ?? "",
      editable: this.dynamic.editable,
      editorProps: {
        attributes: {
          class: "ws-prompt-input__editor",
          role: "textbox",
          "aria-label": props.ariaLabel ?? "Message",
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
          this.emitAction();
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
    this.frame.addEventListener("keydown", (event) => {
      if (this.editor.isEditable) {
        return;
      }
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        this.emitAction();
      }
    });
    this._register(
      toDisposable(() => {
        this.editor.destroy();
      }),
    );
    // The box is its own text-control adapter: the Edit menu's
    // undo/redo/select-all route here whenever the box holds focus. The
    // adapter registers only when the host supplied a registrar and the
    // history plugin is present - without it the commands would no-op,
    // and the native execCommand fallback is the better path.
    // canUndo/canRedo read the history depth so an empty stack falls back
    // instead of swallowing the command.
    const hasHistory = this.editor.extensionManager.extensions.some(
      (extension) => extension.name === "undoRedo",
    );
    if (hasHistory && props.textControls !== undefined) {
      const registration: IDisposable = props.textControls(this.frame, {
        kind: "prosemirror",
        undo: () => {
          this.editor.commands.undo();
        },
        redo: () => {
          this.editor.commands.redo();
        },
        selectAll: () => {
          this.editor.commands.selectAll();
        },
        canUndo: () => undoDepth(this.editor.state) > 0,
        canRedo: () => redoDepth(this.editor.state) > 0,
      });
      this._register(registration);
    }
    this.renderEditable();
    this.renderAction();
    this.renderMic();
    const initialMeasure = window.requestAnimationFrame(() => this.syncHeight());
    this._register(toDisposable(() => window.cancelAnimationFrame(initialMeasure)));
  }

  /** The resolved dynamic props plus the variant, defaults applied. */
  get props(): Readonly<ResolvedDynamicProps & Pick<ChatBoxProps, "variant">> {
    return { ...this.dynamic, variant: this.variant };
  }

  /**
   * Merges a partial set of dynamic props and re-renders only what
   * changed: an unchanged value touches no DOM. Construction-only props
   * are not accepted here by type.
   */
  update(props: Partial<ChatBoxDynamicProps>): void {
    if (props.editable !== undefined && props.editable !== this.dynamic.editable) {
      this.dynamic.editable = props.editable;
      this.renderEditable();
    }
    if (props.action !== undefined && props.action !== this.dynamic.action) {
      this.dynamic.action = props.action;
      this.renderAction();
    }
    if (props.mic !== undefined && props.mic !== this.dynamic.mic) {
      this.dynamic.mic = props.mic;
      this.renderMic();
    }
  }

  /**
   * The send button's press and the submitting Enter share this path:
   * `idle` is silent, `stop` emits `stop`, and both send states emit
   * `send` - `send-blocked` included, so the host can name the blocker.
   */
  private emitAction(): void {
    switch (this.dynamic.action) {
      case "idle":
        return;
      case "stop":
        this.onEvent({ type: "stop" });
        return;
      case "send":
      case "send-blocked":
        this.onEvent({
          type: "send",
          text: this.getText(),
          mentions: this.mentions(),
          attachments: [...this.attachments],
        });
        return;
      default: {
        const exhaustive: never = this.dynamic.action;
        return exhaustive;
      }
    }
  }

  /** The pills in the document, in order, as the chips they were inserted from. */
  private mentions(): ChipRef[] {
    const chips: ChipRef[] = [];
    this.editor.state.doc.descendants((node) => {
      if (node.type.name === "mentionNode") {
        // The schema's own attribute definitions are the only writers,
        // so the open record narrows to what the extension declares.
        chips.push(chipFromAttrs(node.attrs as ChipNodeAttrs));
      }
      return true;
    });
    return chips;
  }

  /** Applies both locks to the editor and mirrors the effective state onto the frame. */
  private renderEditable(): void {
    const effective = this.dynamic.editable && !this.takeReadOnly;
    if (this.editor.isEditable !== effective) {
      this.editor.setEditable(effective);
    }
    this.frame.dataset["editable"] = String(effective);
  }

  private renderAction(): void {
    const action = this.dynamic.action;
    this.send.dataset["action"] = action;
    this.send.disabled = action === "idle";
    this.send.setAttribute("aria-disabled", String(action === "send-blocked"));
  }

  private renderMic(): void {
    const mic = this.dynamic.mic;
    const recording = mic === "recording";
    this.mic.dataset["mic"] = mic;
    this.mic.classList.toggle("ws-stt-mic--recording", recording);
    this.mic.setAttribute("aria-pressed", String(recording));
    this.mic.title = micTitle(mic);
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

  /** Captures the ProseMirror selection and its target-owned insertion policy. */
  insertionContext(): ReturnType<ChatBoxHandle["insertionContext"]> {
    const { from, to } = this.editor.state.selection;
    const document = this.editor.state.doc;
    return {
      range: { start: from, end: to },
      original: document.textBetween(from, to, "\n", "\n"),
      compositionPrefix:
        from === to &&
        to === document.content.size - 1 &&
        /\S$/.test(document.textBetween(0, from, "\n", "\n"))
          ? " "
          : "",
    };
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
   * the frame (`.ws-stt-input--recording`). Composes with the editable
   * prop through the two flag fields.
   */
  setReadOnly(readOnly: boolean): void {
    this.takeReadOnly = readOnly;
    this.renderEditable();
    this.frame.classList.toggle("ws-stt-input--recording", readOnly);
  }

  /** Focuses the editor; a landed dictation final calls it. */
  focus(): void {
    this.editor.commands.focus();
  }

  /** Inserts a pill for `chip` at the cursor, followed by one space. */
  insertMention(chip: ChipRef): void {
    this.editor
      .chain()
      .insertContent([
        {
          type: "mentionNode",
          attrs: {
            id: chip.id,
            label: chip.label,
            mentionSuggestionChar: "@",
            kind: chip.kind ?? null,
            icon: chip.icon ?? null,
            preview: chip.preview ?? null,
            tone: chip.tone ?? null,
            data: chip.data,
          },
        },
        { type: "text", text: " " },
      ])
      .run();
  }

  /** The persisted form: the document plus the strip's attachments. */
  serialize(): SerializedDraft {
    return { v: 1, doc: this.editor.getJSON(), attachments: [...this.attachments] };
  }

  /**
   * Replaces the box with a serialized draft. A draft whose version is
   * missing or unknown is rejected and the box stands unchanged.
   */
  restore(draft: SerializedDraft): void {
    // Persisted data is only typed as far as the reader trusts it.
    const version: unknown = draft.v;
    if (version !== 1) {
      return;
    }
    this.editor.commands.setContent(draft.doc);
    this.attachments = [...draft.attachments];
    this.strip.replaceChildren(...this.attachments.map((chip) => renderChip(chip)));
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
