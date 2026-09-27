// The inline progress bar shared by both UIs: a thin rounded track whose
// fill scales by the --progress custom property, so updates are
// compositor-only (transform, never width). Used for determinate
// readings; a null fraction renders the track empty with no aria value.

import "./progress.css";

/** The mounted bar and its update handle. */
export interface ProgressBar {
  /** The track element (role="progressbar"); the consumer appends it. */
  readonly element: HTMLElement;
  /** Sets the fill fraction, clamped to 0..1; null clears the value. */
  setFraction(fraction: number | null): void;
}

/** Creates an inline progress bar labeled for assistive technology. */
export function createProgressBar(label: string): ProgressBar {
  const element = document.createElement("div");
  element.className = "progress";
  element.setAttribute("role", "progressbar");
  element.setAttribute("aria-label", label);
  element.setAttribute("aria-valuemin", "0");
  element.setAttribute("aria-valuemax", "100");
  const fill = document.createElement("div");
  fill.className = "progress__fill";
  element.append(fill);

  return {
    element,
    setFraction(fraction: number | null): void {
      if (fraction === null) {
        element.removeAttribute("aria-valuenow");
        fill.style.setProperty("--progress", "0");
        return;
      }
      const clamped = Math.min(Math.max(fraction, 0), 1);
      fill.style.setProperty("--progress", String(clamped));
      element.setAttribute("aria-valuenow", String(Math.round(clamped * 100)));
    },
  };
}
