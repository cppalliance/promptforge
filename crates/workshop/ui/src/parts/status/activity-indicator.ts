// The activity LED: an indicator slot the status bar hosts, driven by the
// observer's status frames. Thinking lights it amber and generating
// green, with green winning while both coincide. Every thinking or
// generating frame - debug frames included - pulses it; info and error
// frames also set or clear the sustained state that the pulse decays to.

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import type { StatusIndicatorHandle, StatusIndicators } from "@workshop/platform/status-indicators";
import type { StatusFrame } from "../../services/protocol";

type PulseActivity = "thinking" | "generating";

// Used when the stylesheet's --led-pulse-ms cannot be read (jsdom, or a
// skin that dropped the variable).
const DEFAULT_LED_PULSE_MS = 250;

export class ActivityIndicator extends Disposable {
  private readonly led: StatusIndicatorHandle;
  private readonly lit = new Set<PulseActivity>();
  private sustained: PulseActivity | null = null;
  private ledTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(indicators: StatusIndicators) {
    super();
    this.led = this._register(
      indicators.register({ id: "activity", name: "Activity indicator", order: 1, decorative: true }),
    );
    // The pulse decay timer is the indicator's only other owned resource.
    this._register(
      toDisposable(() => {
        if (this.ledTimer !== null) {
          clearTimeout(this.ledTimer);
          this.ledTimer = null;
        }
      }),
    );
  }

  /** Applies one observer update. Debug frames pulse the LED only. */
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
  }

  /**
   * Turns the LED dark after the persistent socket drops: the sustained
   * state, any pending pulse, and the lit set all clear at once.
   */
  reset(): void {
    this.sustained = null;
    if (this.ledTimer !== null) {
      clearTimeout(this.ledTimer);
      this.ledTimer = null;
    }
    this.lit.clear();
    this.applyLed();
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

  /** Applies the lit set: green wins while generating and thinking coincide. */
  private applyLed(): void {
    if (this.lit.has("generating")) {
      this.led.set("green");
    } else if (this.lit.has("thinking")) {
      this.led.set("amber");
    } else {
      this.led.set(null);
    }
  }

  /** The hold window, tunable from the stylesheet as --led-pulse-ms. */
  private pulseMs(): number {
    const raw = getComputedStyle(document.documentElement).getPropertyValue("--led-pulse-ms").trim();
    const match = /^(\d+(?:\.\d+)?)(ms|s)$/.exec(raw);
    if (match === null || match[1] === undefined) {
      return DEFAULT_LED_PULSE_MS;
    }
    const value = Number.parseFloat(match[1]);
    return match[2] === "s" ? value * 1000 : value;
  }
}
