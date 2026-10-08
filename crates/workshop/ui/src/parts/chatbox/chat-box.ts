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
// through one sink, an imperative handle for the owning part and for
// dictation. It reaches no service registry - the text-control registrar is
// injected - and imports nothing from the embedding layers. Every prop-driven
// state is mirrored onto the DOM as a data attribute beside the classes
// the skin already relies on. The `@` typeahead's items come from the
// injected mentionSource (a three-item stub by default) through the
// ProseMirror suggestion plugin, which owns the debounce, the abort, and
// the stale-result guard; the box only forwards the plugin's signal.
// An `error` prop opens a popup docked on the card's top edge - a warning
// glyph, a title over a message, and an optional Try again, whose press the
// box reports as a `retry` event for the owning part to act on.

import "./chat-box.css";

import { Editor, type JSONContent } from "@tiptap/core";
import { Placeholder } from "@tiptap/extension-placeholder";
import { closeHistory, redoDepth, undoDepth } from "@tiptap/pm/history";
import { Slice } from "@tiptap/pm/model";
import { StarterKit } from "@tiptap/starter-kit";
import type { EditorState } from "@tiptap/pm/state";
import { Disposable, type IDisposable, toDisposable } from "@workshop/platform/lifecycle";
import { ICON_MIC, ICON_SEND, ICON_WARNING } from "@workshop/look/icons";
import { renderChip } from "./chip-view";
import {
  attrsFromChip,
  type ChipNodeAttrs,
  chipFromAttrs,
  MentionChip,
  MentionSuggestionPluginKey,
} from "./mention-chip";
import { setTentativeMark, TentativeMark } from "./tentative-mark";
import { renderMentionTypeahead } from "./typeahead-popup";
import type {
  ChatBoxDynamicProps,
  ChatBoxError,
  ChatBoxEventSink,
  ChatBoxHandle,
  ChatBoxProps,
  ChipRef,
  ChipSource,
  SerializedDraft,
} from "./types";

// The fallbacks mirror the token defaults in @workshop/look/tokens.css; they
// apply when the skin is absent (tests) or the token is deleted.
const DEFAULT_MIN_HEIGHT_PX = 36;
const DEFAULT_MAX_HEIGHT_PX = 200;

// The ProseMirror suggestion plugin waits this long after the last
// keystroke before asking the source, and aborts the in-flight query when
// a newer one arrives; the box adds no timing logic of its own.
const MENTION_DEBOUNCE_MS = 60;

// STUB for the future workspace file index: three canned entries keep
// the popup's open/filter/select cycle working until the owning part
// supplies a mentionSource.
const STUB_CHIPS: readonly ChipRef[] = [
  { id: "README.md", label: "README.md", kind: "file", data: null },
  { id: "src/main.ts", label: "src/main.ts", kind: "file", data: null },
  { id: "Cargo.toml", label: "Cargo.toml", kind: "file", data: null },
];

/**
 * The default `@` source: the stub entries filtered by case-insensitive
 * substring match on the label. Exported so the stub's shape is pinned
 * by a test rather than by the popup it happens to fill.
 */
export const stubMentionSource: ChipSource = (query) => {
  const needle = query.toLowerCase();
  return Promise.resolve(STUB_CHIPS.filter((chip) => chip.label.toLowerCase().includes(needle)));
};

/** The `/` source until commands arrive: nothing, so a typed `/` stays text. */
const NO_COMMANDS: ChipSource = () => Promise.resolve([]);

/**
 * Whether a typeahead session owns the keyboard: editorProps handlers
 * run before the ProseMirror suggestion state plugin's, so the box's
 * Enter and Tab handling must yield while a trigger's session is active
 * or the send would fire instead of the selection. One key today (`@`);
 * the `/` trigger joins this list when it is wired.
 */
function suggestionActive(state: EditorState): boolean {
  return MentionSuggestionPluginKey.getState(state)?.active === true;
}

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

/** The popup's title when the owning part gives none. */
const DEFAULT_ERROR_TITLE = "Connection Error";

/** Whether two error props paint the same popup. */
function sameError(a: ChatBoxError | null, b: ChatBoxError | null): boolean {
  if (a === null || b === null) {
    return a === b;
  }
  return a.title === b.title && a.message === b.message && a.tryAgain === b.tryAgain;
}

/** The popup's parts, kept so an update touches only what changed. */
interface ErrorPopup {
  readonly element: HTMLDivElement;
  readonly title: HTMLDivElement;
  readonly message: HTMLDivElement;
  readonly retry: HTMLButtonElement;
}

/**
 * The chat box: the bar (`ws-agent-session__bar`) holding the framed
 * editor, an optional controls element the owning part supplies, and
 * the box's own mic and send buttons. Disposable: dispose() destroys
 * the editor, releases the text-control registration, and takes its
 * buttons back out of the controls element it was handed.
 *
 * The handle is a structural superset of dictation's input target:
 * dictation splices the transcript in through insertionContext and
 * replaceRange, puts a take's captured selection back through
 * restoreRange, parks the cursor at a take's end through setSelection,
 * styles a take's tentative words through setTentativeRange, and holds
 * the box with setReadOnly. Offsets are ProseMirror positions.
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
  private errorPopup: ErrorPopup | null = null;
  private attachments: ChipRef[] = [];
  // Held for the `/` trigger, which is not wired to a ProseMirror plugin
  // in this plan: a typed `/` stays text. The seam exists so the owning
  // part's source is in place when the command chip arrives.
  private readonly commandSource: ChipSource;

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
      error: props.error ?? null,
    };
    this.commandSource = props.commandSource ?? NO_COMMANDS;
    const mentionSource = props.mentionSource ?? stubMentionSource;

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
    // trail the owning part's toolbar; without one they sit on the bar. The
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
        // undoRedo, whose ProseMirror history plugin backs the text-control
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
          // The gated (non-editable) box still shows its placeholder,
          // same as a disabled textarea: the gate's "the agent is
          // working" message IS the non-editable state.
          showOnlyWhenEditable: false,
        }),
        // Inline mention pills (@-referenced chips) with this box's
        // source and the typeahead popup wired into the extension's
        // suggestion seam. The ProseMirror plugin owns the async handling: it
        // debounces, hands the source an AbortSignal it fires on a
        // newer keystroke, discards a stale resolution, and reports
        // `loading` to the popup. minQueryLength 0 means a bare `@`
        // shows results.
        MentionChip.configure({
          suggestion: {
            items: ({ query, signal }) => mentionSource(query, signal),
            render: renderMentionTypeahead,
            debounce: MENTION_DEBOUNCE_MS,
            minQueryLength: 0,
          },
        }),
        TentativeMark,
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
          // An open typeahead owns Enter and Tab - both insert the highlighted
          // item - so the box yields them to the ProseMirror plugin.
          if ((event.key === "Enter" || event.key === "Tab") && suggestionActive(view.state)) {
            return false;
          }
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
    // read-only - yet an Enter there is still a send, submitting what the
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
    // adapter registers only when the owning part supplied a registrar
    // and the ProseMirror history plugin is present - without it the
    // commands would no-op, and the native execCommand fallback is the
    // better path. canUndo/canRedo read the history depth so an empty
    // stack falls back instead of swallowing the command.
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
    this.renderError();
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
    if (props.error !== undefined && !sameError(props.error, this.dynamic.error)) {
      this.dynamic.error = props.error;
      this.renderError();
    }
  }

  /**
   * The send button's press and the submitting Enter share this path:
   * `idle` is silent, `stop` emits `stop`, and both send states emit
   * `send`, `send-blocked` included, so the owning part can name the blocker.
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

  /**
   * Paints the error popup docked on the card's top edge: the warning
   * glyph, the title (default "Connection Error") over the message, and,
   * when offered, a right-aligned Try again that is disabled until the
   * owning part enables it. The text is the owning part's and lands
   * through textContent.
   */
  private renderError(): void {
    const error = this.dynamic.error;
    if (error === null) {
      this.errorPopup?.element.remove();
      this.errorPopup = null;
      return;
    }
    if (this.errorPopup === null) {
      this.errorPopup = this.buildErrorPopup();
      this.element.prepend(this.errorPopup.element);
    }
    const popup = this.errorPopup;
    popup.title.textContent = error.title ?? DEFAULT_ERROR_TITLE;
    popup.message.textContent = error.message;
    popup.retry.hidden = error.tryAgain === undefined;
    popup.retry.disabled = error.tryAgain !== "enabled";
  }

  private buildErrorPopup(): ErrorPopup {
    const element = document.createElement("div");
    element.className = "ws-chat-error";
    element.setAttribute("role", "alert");
    const icon = document.createElement("span");
    icon.className = "ws-chat-error__icon";
    icon.setAttribute("aria-hidden", "true");
    // A static string from @workshop/look, never data.
    icon.innerHTML = ICON_WARNING;
    const title = document.createElement("div");
    title.className = "ws-chat-error__title";
    const message = document.createElement("div");
    message.className = "ws-chat-error__message";
    const retry = document.createElement("button");
    retry.type = "button";
    retry.className = "button button-secondary button-sm ws-chat-error__retry";
    retry.textContent = "Try again";
    retry.addEventListener("click", () => this.onEvent({ type: "retry" }));
    element.append(icon, title, message, retry);
    return { element, title, message, retry };
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

  /**
   * Captures the ProseMirror selection, its content as slice JSON for
   * restoreRange, and its target-owned insertion policy.
   */
  insertionContext(): ReturnType<ChatBoxHandle["insertionContext"]> {
    const { from, to } = this.editor.state.selection;
    const document = this.editor.state.doc;
    return {
      range: { start: from, end: to },
      original: document.textBetween(from, to, "\n", "\n"),
      content: document.slice(from, to).toJSON(),
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
   * occupies exactly text.length positions. A transient replace stays out
   * of the undo history; any other replace is its own undo step, never
   * merged with an adjacent edit made just before or after it.
   */
  replaceRange(
    from: number,
    to: number,
    text: string,
    options: { readonly transient?: boolean } = {},
  ): void {
    const transient = options.transient === true;
    const chain = this.editor.chain().command(({ tr }) => {
      if (transient) {
        tr.setMeta("addToHistory", false);
      } else {
        closeHistory(tr);
      }
      return true;
    });
    if (text === "") {
      chain.deleteRange({ from, to }).setTextSelection(from).run();
    } else {
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
      chain
        .insertContentAt({ from, to }, content)
        .setTextSelection(from + text.length)
        .run();
    }
    if (!transient) {
      this.editor.view.dispatch(closeHistory(this.editor.state.tr));
    }
  }

  /**
   * Replaces [from, to] with slice JSON an earlier insertionContext
   * captured, outside the undo history, and leaves the cursor after it,
   * so paragraph breaks and pills return exactly.
   */
  restoreRange(from: number, to: number, content: unknown): void {
    const slice = Slice.fromJSON(this.editor.schema, content);
    this.editor
      .chain()
      .command(({ tr }) => {
        tr.setMeta("addToHistory", false).replace(from, to, slice);
        return true;
      })
      .setTextSelection(from + slice.size)
      .run();
  }

  /**
   * Styles one dictation take's tentative words at ProseMirror positions,
   * or clears that take's styling when range is null. The text and the
   * undo history are untouched.
   */
  setTentativeRange(
    takeId: number,
    range: { readonly from: number; readonly to: number } | null,
  ): void {
    this.editor.view.dispatch(setTentativeMark(this.editor.state.tr, takeId, range));
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
          attrs: { ...attrsFromChip(chip), mentionSuggestionChar: "@" },
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
    this.strip.replaceChildren(
      ...this.attachments.map((chip) => renderChip(chip, { removable: false })),
    );
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
