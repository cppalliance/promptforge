// The status bar shell shared by both UIs: a permanent full-width footer
// with a text region on the left and a fixed-width slot on the right that
// holds either the inline progress bar or the indicators group - never
// both. Each UI populates the indicators group with its own LEDs (the
// workshop: recording + activity; the gateway: per-endpoint capability)
// and the extras region with its own controls (the gateway: the model
// summary, the pending-queue count, and the cancel buttons). The shell
// owns no timers, listeners, or polling; the consumer drives it through
// setText and renderSlot and owns every lifecycle.

import "./status-bar.css";

/** One progress reading for the slot's bar. */
export interface SlotProgress {
  readonly current: number;
  readonly total: number;
}

/** Options for {@link StatusBarShell.setText}. */
export interface StatusBarText {
  /** Paint the text in the error color. */
  readonly error?: boolean;
  /** The bar's tooltip; defaults to cleared. */
  readonly tooltip?: string;
}

/** The mounted shell and its regions. */
export interface StatusBarShell {
  /** The `<footer class="status-bar">` element; the consumer appends it. */
  readonly element: HTMLElement;
  /** The left text region. */
  readonly text: HTMLElement;
  /** The slot's progress bar. */
  readonly progress: HTMLProgressElement;
  /** The slot's indicators group; the consumer fills it with its LEDs. */
  readonly indicators: HTMLElement;
  /** The region between the text and the slot for consumer controls. */
  readonly extras: HTMLElement;
  /** Sets the left text, its error styling, and the bar tooltip. */
  setText(label: string, options?: StatusBarText): void;
  /**
   * Swaps the slot between the progress bar and the indicators group.
   * Progress wins: a reading shows the bar and hides the group; null
   * restores the group. The swap rides the `hidden` attribute and never
   * touches the indicators' contents, so a live LED reappears lit; the
   * slot's fixed width keeps the bar from reflowing.
   */
  renderSlot(progress: SlotProgress | null): void;
}

/** Creates the status bar shell. */
export function createStatusBarShell(): StatusBarShell {
  const element = document.createElement("footer");
  element.className = "status-bar";
  element.setAttribute("role", "status");
  element.setAttribute("aria-live", "polite");

  const text = document.createElement("span");
  text.className = "status-bar__text";

  const extras = document.createElement("span");
  extras.className = "status-bar__extras";

  const right = document.createElement("span");
  right.className = "status-bar__right";
  const slot = document.createElement("span");
  slot.className = "status-bar__slot";
  const progress = document.createElement("progress");
  progress.className = "status-bar__progress";
  progress.value = 0;
  progress.max = 100;
  progress.setAttribute("aria-label", "Task progress");
  progress.hidden = true;
  const indicators = document.createElement("span");
  indicators.className = "status-bar__indicators";
  slot.append(progress, indicators);
  right.append(slot);
  element.append(text, extras, right);

  return {
    element,
    text,
    progress,
    indicators,
    extras,
    setText(label: string, options?: StatusBarText): void {
      text.textContent = label;
      element.title = options?.tooltip ?? "";
      text.classList.toggle("status-bar__text--error", options?.error === true);
    },
    renderSlot(value: SlotProgress | null): void {
      if (value) {
        // A zero total is degenerate; clamp so value/max stay valid.
        progress.max = value.total > 0 ? value.total : 1;
        progress.value = value.current;
        progress.hidden = false;
        indicators.hidden = true;
      } else {
        progress.hidden = true;
        indicators.hidden = false;
      }
    },
  };
}
