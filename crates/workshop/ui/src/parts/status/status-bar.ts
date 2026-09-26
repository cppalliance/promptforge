// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the shared status bar view
// (shared-ui/status-bar), which owns the bar, the text region, and the
// busy barberpole beside the indicators. Info and error frames set the
// text (the description shows as the tooltip) and drive the barberpole;
// debug frames are internal instrumentation: they never touch the text
// or the barberpole, but they do pulse the LED. The workshop's
// indicators group holds the recording and activity LEDs; the view's
// extras region stays empty.

import { createStatusBarView, type StatusBarView } from "shared-ui/status-bar";

import { Disposable, toDisposable } from "../../base/lifecycle";
import { CONTEXT_KEY_SERVICE, type ContextKey } from "../../services/context-key-service";
import type { StatusFrame } from "../../services/protocol";
import { getService } from "../../services/service-registry";
import type { StatusBar as StatusBarContract } from "../../services/status-bar";

type PulseActivity = "thinking" | "generating";

// Used when the stylesheet's --led-pulse-ms cannot be read (jsdom, or a
// skin that dropped the variable).
const DEFAULT_LED_PULSE_MS = 250;

export class StatusBar extends Disposable implements StatusBarContract {
  private readonly view: StatusBarView;
  private readonly led: HTMLElement;
  private readonly rec: HTMLElement;
  private readonly lit = new Set<PulseActivity>();
  private sustained: PulseActivity | null = null;
  private ledTimer: ReturnType<typeof setTimeout> | null = null;
  // The bar's own visibility key: the Appearance menu's Status Bar row
  // reads it for its checkbox. Bound here because the bar owns the
  // element; visible by default, matching the boot layout.
  private readonly visibleKey: ContextKey<boolean>;

  constructor() {
    super();
    this.visibleKey = getService(CONTEXT_KEY_SERVICE).createKey("statusBarVisible", true);
    this.view = createStatusBarView();
    // The workshop's indicators: the recording LED has the --rec
    // marker; the activity LED is the unmarked one.
    this.rec = document.createElement("span");
    this.rec.className = "status-bar__led status-bar__led--rec";
    this.rec.setAttribute("aria-label", "Recording indicator");
    this.led = document.createElement("span");
    this.led.className = "status-bar__led";
    this.led.setAttribute("aria-hidden", "true");
    this.view.indicators.append(this.rec, this.led);
    this.view.setText("Ready");
    // The bar is the body's full-width footer, below the desk.
    document.body.append(this.view.element);
    this._register(toDisposable(() => this.view.element.remove()));
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
    this.view.setText(frame.label, {
      tooltip: frame.description,
      error: frame.severity === "error",
    });
    this.view.setBusy(frame.busy);
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
    this.view.setText(label, { error: severity === "error" });
  }

  /** Whether the bar is currently shown. */
  get isVisible(): boolean {
    return !this.view.element.hidden;
  }

  /**
   * Shows or hides the bar (the Appearance menu's Status Bar row),
   * mirroring the state into the statusBarVisible context key so the
   * row's checkbox follows.
   */
  setVisible(visible: boolean): void {
    this.view.element.hidden = !visible;
    this.visibleKey.set(visible);
  }

  /**
   * Clears every LED activity state - sustained and pulsed - and applies
   * the idle lens. Only the activity LED is touched: the text, tooltip,
   * barberpole, and recording LED belong to other flows. Used when a chat is
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
   * drops: neutral text, no tooltip, no error styling, and the barberpole
   * hidden.
   */
  reset(): void {
    this.sustained = null;
    this.view.setText("Reconnecting...");
    this.view.setBusy(false);
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
    if (match === null || match[1] === undefined) {
      return DEFAULT_LED_PULSE_MS;
    }
    const value = Number.parseFloat(match[1]);
    return match[2] === "s" ? value * 1000 : value;
  }
}
