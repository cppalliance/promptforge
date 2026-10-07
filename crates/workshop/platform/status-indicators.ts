// The status indicator slots contract: features register LED-style
// indicators, and clickable non-LED slots they draw into themselves,
// into the status bar's indicators group and drive each one through its
// handle. The Workshop UI's status bar implements it; only this
// interface and the token live here, in the DOM-free platform package.

import type { IDisposable } from "./lifecycle";
import { createServiceToken, type ServiceToken } from "./service-registry";

/** An indicator's lit color; null is the unlit lens. */
export type IndicatorState = "green" | "amber" | "red" | null;

/** One indicator's registration. */
export interface StatusIndicatorOptions {
  /** Unique across the bar; a duplicate throws. */
  readonly id: string;
  /** The accessible label, unless `decorative`. */
  readonly name: string;
  /** Sorts ascending, left to right. */
  readonly order: number;
  /** Hidden from assistive tech instead of labelled. */
  readonly decorative?: boolean;
}

/** A registered indicator; dispose removes it from the bar. */
export interface StatusIndicatorHandle extends IDisposable {
  /** Shows exactly one color, or none for null. A tooltip sets the title and joins the label. */
  set(state: IndicatorState, tooltip?: string): void;
}

/** One non-LED slot's registration. */
export interface StatusSlotOptions {
  /** Unique across the bar, indicators included; a duplicate throws. */
  readonly id: string;
  /** The accessible label. */
  readonly name: string;
  /** Sorts ascending among the indicators, left to right. */
  readonly order: number;
  /** Runs on every click. */
  readonly activate: () => void;
}

/** A registered non-LED slot; dispose removes it from the bar and stops activating. */
export interface StatusSlotHandle extends IDisposable {
  /** The slot's host element, which its owner draws into. */
  readonly element: HTMLElement;
  /** Sets the title, which joins the label; none restores the plain name. */
  setTooltip(tooltip?: string): void;
}

/** The indicator slots, which consumers resolve from the registry. */
export interface StatusIndicators {
  /** Adds one indicator in `order`; throws on a duplicate id. */
  register(options: StatusIndicatorOptions): StatusIndicatorHandle;
  /** Adds one non-LED slot in `order` among the indicators; throws on a duplicate id. */
  registerSlot(options: StatusSlotOptions): StatusSlotHandle;
}

/** The registry token for the indicator slots the composition root registers. */
export const STATUS_INDICATORS: ServiceToken<StatusIndicators> =
  createServiceToken<StatusIndicators>("workshop.statusIndicators");
