// Shared UI contracts and text-target adapters for Realtime dictation.

import "./stt.css";

import type { IDisposable } from "../base/lifecycle";
export { setupStt } from "./realtime-stt";

/** One immutable snapshot of the target-owned transcript insertion policy. */
export interface SttInsertionContext {
  /** The selected range in the target's coordinate space. */
  readonly range: {
    readonly start: number;
    readonly end: number;
  };
  /** The selected text a cancelled or failed take restores. */
  readonly original: string;
  /** The separator owned by this take, if appending requires one. */
  readonly compositionPrefix: "" | " ";
}

/**
 * What dictation needs from its host input: a text target the take can
 * splice the transcript into. Offsets are the target's own text
 * coordinates - a textarea's string offsets, the prompt editor's
 * ProseMirror positions. A take only ever combines a captured `start`
 * with the length of the text it last inserted there, which is valid in
 * both spaces.
 */
export interface SttInputTarget {
  /** Captures the selected range, rollback text, and target-owned insertion policy. */
  insertionContext(): SttInsertionContext;
  /** Replaces [from, to] with text, leaving the cursor after the inserted text. */
  replaceRange(from: number, to: number, text: string): void;
  /** Locks the input against typing while a take splices, or releases it. */
  setReadOnly(readOnly: boolean): void;
  /** Returns focus to the input; a landed final calls it. */
  focus(): void;
}

export interface SttElements {
  mic: HTMLButtonElement;
  input: SttInputTarget;
}

/**
 * The textarea target: one-line wrappers over the native selection API,
 * behavior unchanged from when setupStt held the textarea directly.
 */
export function textareaSttTarget(input: HTMLTextAreaElement): SttInputTarget {
  return {
    insertionContext: () => {
      const start = input.selectionStart ?? input.value.length;
      const end = input.selectionEnd ?? input.value.length;
      return {
        range: { start, end },
        original: input.value.slice(start, end),
        compositionPrefix:
          start === end && end === input.value.length && /\S$/.test(input.value.slice(0, start))
            ? " "
            : "",
      };
    },
    replaceRange: (from, to, text) => {
      input.setRangeText(text, from, to, "end");
      // Programmatic value sets don't fire the textarea's "input" event,
      // so every dictation-driven rewrite dispatches it: dictation
      // behaves like typing to whatever listens on the input.
      input.dispatchEvent(new Event("input", { bubbles: true }));
    },
    setReadOnly: (readOnly) => {
      input.readOnly = readOnly;
      input.classList.toggle("stt-input--recording", readOnly);
    },
    focus: () => input.focus(),
  };
}

/**
 * The status-bar slice dictation paints: local messages (blockers, capture
 * failures, an empty take) and the recording LED. `StatusBar` satisfies it
 * structurally; tests hand in a recording fake.
 */
export interface SttStatus {
  showLocal(label: string, severity: "info" | "error"): void;
  setRecording(on: boolean): void;
}

/**
 * What blocks starting a take right now, as a user-readable reason, or
 * null when a take may start. Consulted on every mic click: the mic stays
 * visible and clickable even when blocked, so the click can name the
 * blocker on the status bar instead of the control silently disappearing.
 */
export type SttBlocker = () => string | null;

/** The per-tab dictation control; dispose() unwires the mic and discards a live take. */
export interface SttHandle extends IDisposable {
  discardIfRecording(): void;
}
