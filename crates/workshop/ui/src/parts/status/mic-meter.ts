// The mic meter: a status bar slot left of the record LED showing the
// shared microphone's loudness while someone dictates. It keeps the last
// METER_BARS chunk levels and draws them newest on the right, one
// animation frame per batch of new levels, only while capture has an
// owner; idle, the bars rest as dots and no frame is requested. The
// tooltip and accessible label name the owner, and a click reveals it.

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import type { StatusIndicators, StatusSlotHandle } from "@workshop/platform/status-indicators";
import type { SpeechCaptureService } from "../../services/speech-capture";

/** How many recent levels the meter shows. */
export const METER_BARS = 9;

export class MicMeter extends Disposable {
  private readonly slot: StatusSlotHandle;
  private readonly bars: HTMLElement[];
  private readonly levels: number[] = new Array<number>(METER_BARS).fill(0);
  private frame: number | null = null;
  private tooltip: string | undefined;

  constructor(
    indicators: StatusIndicators,
    private readonly capture: SpeechCaptureService,
  ) {
    super();
    this.slot = this._register(
      indicators.registerSlot({
        id: "mic-meter",
        name: "Microphone",
        order: -1,
        activate: () => this.capture.presence?.reveal(),
      }),
    );
    this.slot.element.classList.add("status-bar__meter");
    this.bars = Array.from({ length: METER_BARS }, () => {
      const bar = document.createElement("span");
      bar.className = "status-bar__meter-bar";
      return bar;
    });
    this.slot.element.append(...this.bars);
    this._register(toDisposable(() => this.cancelFrame()));
    this._register(capture.onLevel((level) => this.push(level)));
    this._register(capture.onOwnerChange(() => this.sync()));
    this.sync();
  }

  /** Scrolls in one owned level and asks for a frame unless one is pending. */
  private push(level: number): void {
    if (this.capture.presence === null) {
      return;
    }
    this.levels.shift();
    this.levels.push(level);
    this.frame ??= window.requestAnimationFrame(() => {
      this.frame = null;
      this.draw();
    });
  }

  /** Restarts from silence for a new owner, or rests as idle dots without one. */
  private sync(): void {
    this.cancelFrame();
    this.levels.fill(0);
    this.slot.element.classList.toggle("status-bar__meter--idle", this.capture.presence === null);
    this.draw();
  }

  /** Paints every bar's level and refreshes the tooltip, so a renamed owner shows. */
  private draw(): void {
    this.bars.forEach((bar, index) => {
      bar.style.setProperty("--level", String(this.levels[index] ?? 0));
    });
    const presence = this.capture.presence;
    const tooltip = presence === null ? undefined : `Dictating to ${presence.label()}`;
    if (tooltip !== this.tooltip) {
      this.tooltip = tooltip;
      this.slot.setTooltip(tooltip);
    }
  }

  private cancelFrame(): void {
    if (this.frame !== null) {
      window.cancelAnimationFrame(this.frame);
      this.frame = null;
    }
  }
}
