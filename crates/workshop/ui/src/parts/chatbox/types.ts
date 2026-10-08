// The chat box contract: what the isolated component takes in (props),
// gives out (events), and exposes (the handle), plus the chip model and
// the persisted draft shape. Everything the owning part and the component
// share is declared here and nowhere else. `chatbox/` imports only
// `@workshop/platform/lifecycle`, `@workshop/look/icons`, the shared menu surface
// `@workshop/look/dropdown.css`, and `@tiptap/*`; the two outside types
// this file mirrors - the text-control adapter and dictation's input
// target - are declared structurally so neither side imports the other.

import type { JSONContent } from "@tiptap/core";
import type { IDisposable } from "@workshop/platform/lifecycle";

/** Any JSON value: the shape of the opaque payload the owning part puts on a chip. */
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

/**
 * One chip: an inline mention pill, a typeahead row, or an attachment.
 * The owning part owns the vocabulary of `kind` and everything inside
 * `data`; the component draws from the rest and round-trips `data` untouched.
 */
export interface ChipRef {
  readonly id: string;
  readonly label: string;
  /**
   * What the chip is (file, folder, command, image, url, ...). The pill
   * renders it as `data-kind` so the skin can style by kind; absent
   * means a generic pill.
   */
  readonly kind?: string;
  /** Dimmed row detail in the typeahead (for files, the parent path); never rendered on the pill. */
  readonly description?: string;
  /** Typeahead section; items sort by group with a header at each boundary. */
  readonly group?: string;
  /** A named icon; the component falls back to an extension map, then a generic glyph. */
  readonly icon?: string;
  /** A thumbnail URL the owning part issues (deferred). */
  readonly preview?: string;
  /** Display state (expired and uploading are deferred). */
  readonly tone?: "default" | "expired" | "uploading";
  /** The owning part's opaque payload, round-tripped untouched. */
  readonly data: JsonValue;
}

/** An async chip provider for a typeahead trigger; `signal` aborts a superseded query. */
export type ChipSource = (query: string, signal: AbortSignal) => Promise<ChipRef[]>;

/**
 * The persisted form of the box. `v` is the schema version: a future
 * change bumps it, and `restore` must read every prior version or
 * reject with the box unchanged.
 */
export interface SerializedDraft {
  readonly v: 1;
  /** The document: text plus inline chips (mentions, later commands). */
  readonly doc: JSONContent;
  /** The strip above the text (empty in this plan). */
  readonly attachments: ChipRef[];
}

/** The agent modes the action button's fill follows. */
export type ChatBoxMode = "agent" | "plan" | "ask" | "debug" | "multitask";

/**
 * The error popup docked on the card's top edge: a warning glyph, a title
 * over a message, and an optional Try again. The owning part decides what
 * Try again does; the box only reports the press.
 */
export interface ChatBoxError {
  /** The popup title; default "Connection Error". */
  readonly title?: string;
  readonly message: string;
  /** Offers Try again, enabled or shown disabled; absent: no button. */
  readonly tryAgain?: "enabled" | "disabled";
}

/**
 * Construction props. Every prop is optional with a stated default, so
 * `new ChatBox()` constructs a working box. The first group is dynamic
 * and may change through `update()`; the rest is construction-only and
 * read once.
 */
export interface ChatBoxProps {
  // dynamic
  /** The error popup's content, or null for no popup; default null. */
  readonly error?: ChatBoxError | null;
  /** Whether the operator can type; default true. */
  readonly editable?: boolean;
  /**
   * What the round action button does once the box holds text; default
   * "send". `send`: enabled. `send-blocked`: aria-disabled but still
   * clickable and still emits `send`, so the owning part can name the
   * blocker. `idle`: disabled. `stop`: the agent is generating - the button
   * becomes Stop and emits `stop`, and the placeholder reads "Add a
   * follow-up". With an empty box and no `stop`, the same button is the mic.
   */
  readonly action?: "send" | "send-blocked" | "stop" | "idle";
  /**
   * The mic's state, shown on the action button while the box is empty (or
   * recording); default "idle".
   */
  readonly mic?: "idle" | "recording" | "blocked";
  /** The agent mode that picks the action button's fill; default "agent". */
  readonly mode?: ChatBoxMode;
  // construction-only
  /**
   * The layout variant, rendered as `data-variant` on the root; default
   * "expanded". Reserved so later editors add values, not props.
   */
  readonly variant?: "expanded";
  /**
   * Placeholder while the editor is empty; default "". The function
   * form is re-evaluated on every state update.
   */
  readonly placeholder?: string | (() => string);
  /** Accessible label on the editable region; default "Message". */
  readonly ariaLabel?: string;
  /** Initial content, parsed as HTML (`<p>` per paragraph). */
  readonly content?: string;
  /**
   * A toolbar element the owning part supplies, placed after the editor;
   * the box appends its round action button to its end. Absent, the
   * button goes directly on the bar.
   */
  readonly controls?: HTMLElement;
  /** The `@` provider; default: the built-in three-item stub. */
  readonly mentionSource?: ChipSource;
  /**
   * The `/` provider; default: `async () => []`. Reserved: declared but
   * not read in this release; a typed `/` stays text.
   */
  readonly commandSource?: ChipSource;
  /**
   * Turns pasted files into attachment chips; absent: ProseMirror's
   * default paste. Reserved: declared but not read in this release;
   * paste is ProseMirror's default.
   */
  readonly onPasteFiles?: (files: File[]) => Promise<ChipRef[]>;
  /** The owning part's text-control registrar; replaces the service-registry lookup. */
  readonly textControls?: TextControlRegistrar;
}

/**
 * The edit surface the box registers with the owning part's text-control
 * service. Declared structurally: it mirrors `TextControl` in
 * `@workshop/platform/text-control-service` field for field so the
 * owning part's bound `register` type-checks here without an import
 * across the boundary.
 */
export interface ChatBoxTextControl {
  readonly kind: string;
  undo(): void;
  redo(): void;
  selectAll(): void;
  canUndo?(): boolean;
  canRedo?(): boolean;
}

/** Registers `control` for `root`'s subtree; disposing unregisters it. */
export type TextControlRegistrar = (root: HTMLElement, control: ChatBoxTextControl) => IDisposable;

/** Everything the box tells its owning part. */
export type ChatBoxEvent =
  | { readonly type: "send"; readonly text: string; readonly mentions: ChipRef[]; readonly attachments: ChipRef[] }
  | { readonly type: "command"; readonly command: ChipRef; readonly args: string }
  | { readonly type: "stop" }
  | { readonly type: "retry" }
  | { readonly type: "cancel" }
  | { readonly type: "mic-press" }
  | { readonly type: "mic-release" };

/** The owning part's event sink. */
export type ChatBoxEventSink = (event: ChatBoxEvent) => void;

/**
 * The imperative surface. A structural superset of dictation's
 * `SttInputTarget` (insertionContext, replaceRange, restoreRange,
 * setSelection, setTentativeRange, setReadOnly, focus), so
 * `setupStt({ input: handle })` type-checks with no import in either
 * direction.
 */
export interface ChatBoxHandle {
  clear(): void;
  focus(): void;
  getText(): string;
  setText(text: string): void;
  insertMention(chip: ChipRef): void;
  replaceRange(
    from: number,
    to: number,
    text: string,
    options?: { readonly transient?: boolean },
  ): void;
  restoreRange(from: number, to: number, content: unknown): void;
  setSelection(from: number, to: number): void;
  setTentativeRange(takeId: number, range: { readonly from: number; readonly to: number } | null): void;
  insertionContext(): {
    readonly range: { readonly start: number; readonly end: number };
    readonly original: string;
    readonly content: unknown;
    readonly compositionPrefix: "" | " ";
  };
  setReadOnly(readOnly: boolean): void;
  syncHeight(): void;
  serialize(): SerializedDraft;
  restore(draft: SerializedDraft): void;
}

/** The dynamic subset: the only props `update()` accepts. */
export type ChatBoxDynamicProps = Pick<ChatBoxProps, "editable" | "action" | "mic" | "mode" | "error">;
