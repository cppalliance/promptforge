// The inline progress bar for the Workshop family's UIs: a thin track whose
// fill scales by the --progress custom property, so updates are
// compositor-only (transform, never width). Used for determinate readings;
// a null fraction renders the indeterminate bar, a short fill that slides
// along the track, with no aria value.
//
// Two variants, both from Cursor: the settings bar (the default - a 4px
// rounded bar, as on Cursor's settings pages) and the workbench bar (a 2px
// square-ended green line, as under the editor tabs).

import "./progress.css";

/** The two progress variants. */
export type ProgressVariant = "settings" | "workbench";

/** The mounted bar and its update handle. */
export interface ProgressBar {
  /** The track element (role="progressbar"); the consumer appends it. */
  readonly element: HTMLElement;
  /** Sets the fill fraction, clamped to 0..1; null shows the indeterminate bar. */
  setFraction(fraction: number | null): void;
}

/** Creates an inline progress bar labeled for assistive technology. */
export function createProgressBar(label: string, variant: ProgressVariant = "settings"): ProgressBar {
  const element = document.createElement("div");
  element.className = variant === "workbench" ? "progress progress--workbench" : "progress";
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
        element.classList.add("progress--indeterminate");
        return;
      }
      element.classList.remove("progress--indeterminate");
      const clamped = Math.min(Math.max(fraction, 0), 1);
      fill.style.setProperty("--progress", String(clamped));
      element.setAttribute("aria-valuenow", String(Math.round(clamped * 100)));
    },
  };
}
