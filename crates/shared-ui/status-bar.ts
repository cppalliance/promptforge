// The status bar shell shared by both UIs: a permanent full-width footer
// with a text region on the left and, on the right, a barberpole beside
// the indicators group. The barberpole is an indeterminate busy signal:
// it shows while work is in flight and hides otherwise, and it never
// displaces the indicators - the LEDs stay visible either way. Each UI
// populates the indicators group with its own LEDs (the workshop:
// recording + activity; the gateway: per-endpoint capability) and the
// extras region with its own controls (the gateway: the model summary,
// the pending-queue count, and the cancel buttons). The shell owns no
// timers, listeners, or polling; the consumer drives it through setText
// and setBusy and owns every lifecycle.

import "./status-bar.css";

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
  /** The animated busy barberpole; hidden while idle. */
  readonly barberpole: HTMLElement;
  /** The indicators group; the consumer fills it with its LEDs. */
  readonly indicators: HTMLElement;
  /** The region between the text and the right group for consumer controls. */
  readonly extras: HTMLElement;
  /** Sets the left text, its error styling, and the bar tooltip. */
  setText(label: string, options?: StatusBarText): void;
  /**
   * Shows or hides the barberpole. The toggle sets the `hidden`
   * attribute on the barberpole alone and never touches the indicators
   * group or its contents, so a live LED keeps glowing beside it.
   */
  setBusy(busy: boolean): void;
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
  // An indeterminate progressbar: role without aria-valuenow tells
  // assistive tech that work is in flight with no known fraction.
  const barberpole = document.createElement("span");
  barberpole.className = "status-bar__barberpole";
  barberpole.setAttribute("role", "progressbar");
  barberpole.setAttribute("aria-label", "Busy");
  barberpole.hidden = true;
  const slot = document.createElement("span");
  slot.className = "status-bar__slot";
  const indicators = document.createElement("span");
  indicators.className = "status-bar__indicators";
  slot.append(indicators);
  // The barberpole sits immediately before the indicators group.
  right.append(barberpole, slot);
  element.append(text, extras, right);

  return {
    element,
    text,
    barberpole,
    indicators,
    extras,
    setText(label: string, options?: StatusBarText): void {
      text.textContent = label;
      element.title = options?.tooltip ?? "";
      text.classList.toggle("status-bar__text--error", options?.error === true);
    },
    setBusy(busy: boolean): void {
      barberpole.hidden = !busy;
    },
  };
}
