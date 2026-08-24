// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the bar. Info and error frames set
// the text (the description rides as the tooltip) and drive the right slot,
// which holds the progress bar or the activity LED - never both. Debug
// frames are internal instrumentation: they never touch the text or the
// slot.

import type { StatusFrame } from "./workbench-socket";

export class StatusBar {
  private readonly text: HTMLElement;
  private readonly progress: HTMLProgressElement;
  private readonly led: HTMLElement;

  constructor(private readonly root: HTMLElement) {
    const text = root.querySelector<HTMLElement>(".status-bar__text");
    const progress = root.querySelector<HTMLProgressElement>(".status-bar__progress");
    const led = root.querySelector<HTMLElement>(".status-bar__led");
    if (!text || !progress || !led) {
      throw new Error(
        "DOM Error: the status bar is missing its text, progress, or LED element.",
      );
    }
    this.text = text;
    this.progress = progress;
    this.led = led;
  }

  /** Paints one observer update. Debug frames are dropped by the UI. */
  render(frame: StatusFrame): void {
    if (frame.severity === "debug") {
      return;
    }
    this.text.textContent = frame.label;
    this.root.title = frame.description;
    this.text.classList.toggle("status-bar__text--error", frame.severity === "error");
    this.renderSlot(frame.progress);
  }

  /**
   * Swaps the slot between the progress bar and the LED. Progress wins: a
   * frame carrying progress shows the bar at that fraction and hides the
   * LED; a null progress restores the LED. The swap rides the `hidden`
   * attribute, so the slot's fixed width keeps the bar from reflowing.
   */
  private renderSlot(progress: StatusFrame["progress"]): void {
    if (progress) {
      // A zero total is degenerate; clamp so value/max stay valid.
      this.progress.max = progress.total > 0 ? progress.total : 1;
      this.progress.value = progress.current;
      this.progress.hidden = false;
      this.led.hidden = true;
    } else {
      this.progress.hidden = true;
      this.led.hidden = false;
    }
  }
}
