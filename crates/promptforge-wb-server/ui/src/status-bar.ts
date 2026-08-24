// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the bar. Info and error frames set
// the text (the description rides as the tooltip); debug frames are
// internal instrumentation and never touch the text.

import type { StatusFrame } from "./workbench-socket";

export class StatusBar {
  private readonly text: HTMLElement;

  constructor(private readonly root: HTMLElement) {
    const text = root.querySelector<HTMLElement>(".status-bar__text");
    if (!text) {
      throw new Error("DOM Error: .status-bar__text not found inside the status bar.");
    }
    this.text = text;
  }

  /** Paints one observer update. Debug frames are dropped by the UI. */
  render(frame: StatusFrame): void {
    if (frame.severity === "debug") {
      return;
    }
    this.text.textContent = frame.label;
    this.root.title = frame.description;
    this.text.classList.toggle("status-bar__text--error", frame.severity === "error");
  }
}
