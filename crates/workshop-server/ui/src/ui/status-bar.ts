// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the shared status bar shell
// (shared-ui/status-bar), which owns the bar, the text region, and the
// slot's progress/indicators swap. Info and error frames set the text
// (the description rides as the tooltip) and drive the slot; debug frames
// are internal instrumentation: they never touch the text or the slot,
// but they do pulse the LED. The workshop's indicators group holds the
// recording and activity LEDs; the shell's extras region stays empty.

import { createStatusBarShell, type StatusBarShell } from "shared-ui/status-bar";

import { Disposable, toDisposable } from "../base/lifecycle";
import type { StatusFrame } from "../services/protocol";

type PulseActivity = "thinking" | "generating";

// Used when the stylesheet's --led-pulse-ms cannot be read (jsdom, or a
// skin that dropped the variable).
const DEFAULT_LED_PULSE_MS = 250;

export class StatusBar extends Disposable {
  private readonly shell: StatusBarShell;
  private readonly led: HTMLElement;
  private readonly rec: HTMLElement;
  private readonly lit = new Set<PulseActivity>();
  private sustained: PulseActivity | null = null;
  private ledTimer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    super();
    this.shell = createStatusBarShell();
    // The workshop's indicators: the recording LED carries the --rec
    // marker; the activity LED is the unmarked one.
    this.rec = document.createElement("span");
    this.rec.className = "status-bar__led status-bar__led--rec";
    this.rec.setAttribute("aria-label", "Recording indicator");
    this.led = document.createElement("span");
    this.led.className = "status-bar__led";
    this.led.setAttribute("aria-hidden", "true");
    this.shell.indicators.append(this.rec, this.led);
    this.shell.setText("Ready");
    // The bar is the body's full-width footer, below the shell.
    document.body.append(this.shell.element);
    this._register(toDisposable(() => this.shell.element.remove()));
    // The pulse decay timer is the bar's only other owned resource.
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
    this.shell.setText(frame.label, {
      tooltip: frame.description,
      error: frame.severity === "error",
    });
    this.shell.renderSlot(frame.progress);
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

  /** Shows a locally-originated message (e.g. dictation errors). The next observer frame overwrites it. */
  showLocal(label: string, severity: "info" | "error"): void {
    this.shell.setText(label, { error: severity === "error" });
  }

  /**
   * Clears every LED activity state - sustained and pulsed - and applies
   * the idle lens. Only the activity LED is touched: the text, tooltip,
   * progress, and recording LED belong to other flows. Used when a chat is
   * aborted, because the recycled socket never sees the server's terminal
   * status frame for the aborted chat.
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

  /** Lights or dims the recording LED with the mic's recording state. */
  setRecording(on: boolean): void {
    this.rec.classList.toggle("status-bar__led--recording", on);
  }

  /**
   * Returns the bar to its reconnecting state after the persistent socket
   * drops: neutral text, no tooltip, no error styling, and the indicators
   * group back in the slot.
   */
  reset(): void {
    this.sustained = null;
    this.shell.setText("Reconnecting...");
    this.shell.renderSlot(null);
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
