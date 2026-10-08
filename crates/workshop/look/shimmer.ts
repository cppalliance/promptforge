// The shared shimmer driver: turns the .ws-shimmer-text sweep from
// shimmer.css on or off for one element and keeps every sweep in phase.
//
// The sweep is a CSS animation, and an element that re-renders (a dock tab
// title, a transcript row rebuilt on a stream delta) would restart it from
// the left. setShimmer instead sets a negative animation-delay measured from
// one module start time, `-((now - start) mod duration)`, so an animation
// that begins at any moment is already where the shared clock says it
// should be: two elements started at different times sweep in step, and a
// rebuilt element picks up exactly where the one it replaces was.
//
// The duration is the element's own loop: its --shimmer-duration (inline,
// then computed), else the 1s of a transcript tone modifier (a detached
// element has no computed style yet), else the stylesheet's 2s default.

import "./shimmer.css";

/** The class that carries the sweep. */
const SHIMMER_CLASS = "ws-shimmer-text";

/** The stylesheet's default loop, in milliseconds. */
const DEFAULT_DURATION_MS = 2000;

/** The transcript tone modifiers and their loops; shimmer.css sets the same 1s. */
const TONE_DURATIONS_MS: Readonly<Record<string, number>> = {
  "ws-shimmer-text--status": 1000,
  "ws-shimmer-text--thinking": 1000,
  "ws-shimmer-text--tool": 1000,
};

// The one clock every delay is measured from.
const START_MS = Date.now();

/** Parses a CSS time ("1s", "500ms") to milliseconds, or null when it is not a positive time. */
function parseDuration(value: string): number | null {
  const match = /^\s*(\d*\.?\d+)(ms|s)\s*$/.exec(value);
  if (match === null) {
    return null;
  }
  const amount = Number.parseFloat(match[1] ?? "");
  const ms = match[2] === "s" ? amount * 1000 : amount;
  return ms > 0 ? ms : null;
}

/** The element's loop length in milliseconds. */
function durationOf(element: HTMLElement): number {
  const inline = parseDuration(element.style.getPropertyValue("--shimmer-duration"));
  if (inline !== null) {
    return inline;
  }
  const view = element.ownerDocument.defaultView;
  const computed = parseDuration(view?.getComputedStyle(element).getPropertyValue("--shimmer-duration") ?? "");
  if (computed !== null) {
    return computed;
  }
  for (const [className, ms] of Object.entries(TONE_DURATIONS_MS)) {
    if (element.classList.contains(className)) {
      return ms;
    }
  }
  return DEFAULT_DURATION_MS;
}

/**
 * Turns the shimmer on or off for `element`. Turning it on takes the shared
 * phase; calling it again on a running element changes nothing, because
 * moving the delay of a running animation would make the sweep jump. Turning
 * it off clears the class and the delay.
 */
export function setShimmer(element: HTMLElement, on: boolean): void {
  if (!on) {
    element.classList.remove(SHIMMER_CLASS);
    element.style.animationDelay = "";
    return;
  }
  if (element.classList.contains(SHIMMER_CLASS) && element.style.animationDelay !== "") {
    return;
  }
  element.classList.add(SHIMMER_CLASS);
  const duration = durationOf(element);
  const elapsed = (((Date.now() - START_MS) % duration) + duration) % duration;
  element.style.animationDelay = `-${elapsed}ms`;
}
