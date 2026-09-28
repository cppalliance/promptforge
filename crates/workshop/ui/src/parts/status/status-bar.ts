// The status bar renderer: consumes the observer's status frames off the
// persistent socket and paints them into the shared status bar view
// (@workshop/look/status-bar), which owns the bar, the text region, and the
// busy barberpole beside the indicators. Info and error frames set the
// text (the description shows as the tooltip) and drive the barberpole;
// debug frames are internal instrumentation that never touch either. The
// bar hosts the indicator slots: features register their LEDs into the
// indicators group through StatusIndicators; the view's extras region
// stays empty.

import { createStatusBarView, type StatusBarView } from "@workshop/look/status-bar";

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import { CONTEXT_KEY_SERVICE, type ContextKey } from "@workshop/platform/context-key-service";
import type { StatusFrame } from "../../services/protocol";
import { getService } from "@workshop/platform/service-registry";
import type { StatusBar as StatusBarContract } from "@workshop/platform/status-bar";
import type {
  IndicatorState,
  StatusIndicatorHandle,
  StatusIndicatorOptions,
  StatusIndicators,
} from "@workshop/platform/status-indicators";

const LED_COLORS = ["green", "amber", "red"] as const;

interface Slot {
  readonly element: HTMLElement;
  readonly order: number;
}

export class StatusBar extends Disposable implements StatusBarContract, StatusIndicators {
  private readonly view: StatusBarView;
  private readonly slots = new Map<string, Slot>();
  private readonly recording: StatusIndicatorHandle;
  // The bar's own visibility key: the Appearance menu's Status Bar row
  // reads it for its checkbox. Bound here because the bar owns the
  // element; visible by default, matching the boot layout.
  private readonly visibleKey: ContextKey<boolean>;

  constructor() {
    super();
    this.visibleKey = getService(CONTEXT_KEY_SERVICE).createKey("statusBarVisible", true);
    this.view = createStatusBarView();
    this.recording = this._register(
      this.register({ id: "recording", name: "Recording indicator", order: 0 }),
    );
    this.view.setText("Ready");
    // The bar is the body's full-width footer, below the desk.
    document.body.append(this.view.element);
    this._register(toDisposable(() => this.view.element.remove()));
  }

  /** Paints one observer update's text, tooltip, and barberpole. */
  render(frame: StatusFrame): void {
    if (frame.severity === "debug") {
      return;
    }
    this.view.setText(frame.label, {
      tooltip: frame.description,
      error: frame.severity === "error",
    });
    this.view.setBusy(frame.busy);
  }

  /**
   * Adds one LED to the indicators group, before the leftmost slot with a
   * higher order, so equal orders keep registration order.
   */
  register(options: StatusIndicatorOptions): StatusIndicatorHandle {
    const { id, name, order } = options;
    if (this.slots.has(id)) {
      throw new Error(`status indicator '${id}' is already registered; ids must be unique`);
    }
    const element = document.createElement("span");
    element.className = "status-bar__led";
    element.dataset.indicator = id;
    const label = (tooltip: string | undefined): void => {
      if (options.decorative === true) {
        element.setAttribute("aria-hidden", "true");
      } else {
        element.setAttribute("aria-label", tooltip ? `${name}: ${tooltip}` : name);
      }
    };
    label(undefined);
    const higher = new Set<Element>(
      [...this.slots.values()].filter((slot) => slot.order > order).map((slot) => slot.element),
    );
    const next = [...this.view.indicators.children].find((child) => higher.has(child)) ?? null;
    this.view.indicators.insertBefore(element, next);
    const slot: Slot = { element, order };
    this.slots.set(id, slot);
    return {
      set: (state: IndicatorState, tooltip?: string): void => {
        for (const color of LED_COLORS) {
          element.classList.remove(`status-bar__led--${color}`);
        }
        if (state !== null) {
          element.classList.add(`status-bar__led--${state}`);
        }
        element.title = tooltip ?? "";
        label(tooltip);
      },
      dispose: (): void => {
        if (this.slots.get(id) === slot) {
          this.slots.delete(id);
          element.remove();
        }
      },
    };
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

  /** Lights or dims the recording LED with the mic's recording state. */
  setRecording(on: boolean): void {
    this.recording.set(on ? "red" : null);
  }

  /**
   * Returns the bar to its reconnecting state after the persistent socket
   * drops: neutral text, no tooltip, no error styling, and the barberpole
   * hidden. Indicators belong to their owners and stay as they are.
   */
  reset(): void {
    this.view.setText("Reconnecting...");
    this.view.setBusy(false);
  }
}
