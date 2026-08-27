// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the bar. Info and error frames set
// the text (the description rides as the tooltip) and drive the right slot,
// which holds the progress bar or the activity LED - never both. Debug
// frames are internal instrumentation: they never touch the text or the
// slot, but they do pulse the LED.

import "./status-bar.css";

import { Disposable, toDisposable } from "../base/lifecycle";
import type { StatusFrame } from "../services/protocol";

type PulseActivity = "thinking" | "generating";

// Used when the stylesheet's --led-pulse-ms cannot be read (jsdom, or a
// skin that dropped the variable).
const DEFAULT_LED_PULSE_MS = 250;

export class StatusBar extends Disposable {
  private readonly text: HTMLElement;
  private readonly progress: HTMLProgressElement;
  private readonly led: HTMLElement;
  private readonly rec: HTMLElement;
  private readonly lit = new Set<PulseActivity>();
  private sustained: PulseActivity | null = null;
  private ledTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly root: HTMLElement) {
    super();
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
    // The pulse decay timer is the bar's only owned resource.
    this._register(
      toDisposable(() => {
        if (this.ledTimer !== null) {
          clearTimeout(this.ledTimer);
          this.ledTimer = null;
        }
      }),
    );
  }

  /** Paints one observer update. Debug frames pulse the LED only. */
  render(frame: StatusFrame): void {
    if (frame.activity === "thinking" || frame.activity === "generating") {
      this.pulse(frame.activity);
    }
    if (frame.severity === "debug") {
      return;
    }
    // Info/error frames set or clear the sustained LED state. Thinking
    // keeps the amber LED lit until something else takes over; any other
    // activity clears it so the LED returns to idle after the pulse decays.
    this.sustained = frame.activity === "thinking" ? "thinking" : null;
    // With no pulse pending, nothing else will ever repaint the LED - an
    // earlier pulse's decay may have re-added the old sustained state to
    // the lit set and cleared the timer, orphaning that glow. Land the lit
    // set on the new sustained value here. A pending pulse needs no help:
    // its decay already lands on the updated sustained state.
    if (this.ledTimer === null) {
      this.lit.clear();
      if (this.sustained) this.lit.add(this.sustained);
      this.applyLed();
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
      if (this.sustained) this.lit.add(this.sustained);
      this.applyLed();
      this.ledTimer = null;
    }, this.pulseMs());
  }

  /** Shows a locally-originated message (e.g. voice capture errors). The next observer frame overwrites it. */
  showLocal(label: string, severity: "info" | "error"): void {
    this.text.textContent = label;
    this.root.title = "";
    this.text.classList.toggle("status-bar__text--error", severity === "error");
  }

  /**
   * Clears every LED activity state - sustained and pulsed - and applies
   * the idle lens. Only the LED is touched: the text, tooltip, progress,
   * and REC badge belong to other flows. Used when a chat is aborted,
   * because the recycled socket never sees the server's terminal status
   * frame for the aborted chat.
   */
  clearActivity(): void {
    this.sustained = null;
    this.lit.clear();
    if (this.ledTimer !== null) {
      clearTimeout(this.ledTimer);
      this.ledTimer = null;
    }
    this.applyLed();
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
    this.sustained = null;
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
