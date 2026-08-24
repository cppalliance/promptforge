// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the bar. Info and error frames set
// the text (the description rides as the tooltip) and drive the right slot,
// which holds the progress bar or the activity LED - never both. Debug
// frames are internal instrumentation: they never touch the text or the
// slot, but they do pulse the LED.

import type { StatusFrame } from "./workbench-socket";

type PulseActivity = "thinking" | "generating";

// Used when the stylesheet's --led-pulse-ms cannot be read (jsdom, or a
// skin that dropped the variable).
const DEFAULT_LED_PULSE_MS = 250;

export class StatusBar {
  private readonly text: HTMLElement;
  private readonly progress: HTMLProgressElement;
  private readonly led: HTMLElement;
  private readonly rec: HTMLElement;
  private readonly lit = new Set<PulseActivity>();
  private ledTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly root: HTMLElement) {
    const text = root.querySelector<HTMLElement>(".status-bar__text");
    const progress = root.querySelector<HTMLProgressElement>(".status-bar__progress");
    const led = root.querySelector<HTMLElement>(".status-bar__led");
    const rec = root.querySelector<HTMLElement>(".status-bar__rec");
    if (!text || !progress || !led || !rec) {
      throw new Error(
        "DOM Error: the status bar is missing its text, progress, LED, or REC element.",
      );
    }
    this.text = text;
    this.progress = progress;
    this.led = led;
    this.rec = rec;
  }

  /** Paints one observer update. Debug frames pulse the LED only. */
  render(frame: StatusFrame): void {
    if (frame.activity === "thinking" || frame.activity === "generating") {
      this.pulse(frame.activity);
    }
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

  /**
   * Lights the LED for one pulse window. JS only toggles a modifier class;
   * the glow and its fades are pure CSS (the modifier's transition is a
   * fast fade-in, the idle rule's transition is the ~--led-pulse-ms
   * ease-out decay). One shared hold timer: any pulse re-arms the window,
   * so a stream of pulses reads as one continuous glow that fades when the
   * activity stops.
   */
  private pulse(activity: PulseActivity): void {
    this.lit.add(activity);
    this.applyLed();
    if (this.ledTimer !== null) {
      clearTimeout(this.ledTimer);
    }
    this.ledTimer = setTimeout(() => {
      this.lit.clear();
      this.applyLed();
      this.ledTimer = null;
    }, this.pulseMs());
  }

  /** Lights or dims the REC badge with the mic's recording state. */
  setRecording(on: boolean): void {
    this.rec.classList.toggle("status-bar__rec--active", on);
  }

  /**
   * Returns the bar to its reconnecting state after the persistent socket
   * drops: neutral text, no tooltip, no error styling, and the LED back in
   * the slot.
   */
  reset(): void {
    this.text.textContent = "Reconnecting...";
    this.root.title = "";
    this.text.classList.remove("status-bar__text--error");
    this.renderSlot(null);
  }

  /** Applies the lit set: green wins while generating and thinking coincide. */
  private applyLed(): void {
    const generating = this.lit.has("generating");
    this.led.classList.toggle("status-bar__led--generating", generating);
    this.led.classList.toggle(
      "status-bar__led--thinking",
      !generating && this.lit.has("thinking"),
    );
  }

  /** The hold window, tunable from the stylesheet as --led-pulse-ms. */
  private pulseMs(): number {
    const raw = getComputedStyle(this.led).getPropertyValue("--led-pulse-ms").trim();
    const match = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(raw);
    if (!match) {
      return DEFAULT_LED_PULSE_MS;
    }
    const value = Number.parseFloat(match[1]);
    return match[2] === "s" ? value * 1000 : value;
  }
}
