// Full-screen switch/apply overlay [Adapted: Unsloth]: a dimmed layer
// centering a card that lists the gateway's SSE stages, each with a
// spinner while active, a check when passed, and an error mark when the
// switch dies in it. The terminal event closes the overlay - instantly
// on success, after a short hold on failure so the failed stage is seen
// (the toast carries the message onward).

import { Check, X, createElement as lucideElement } from "lucide";

import { scheduleTimeout } from "./toast";

/** How long a failed overlay stays up before removing itself. */
const ERROR_HOLD_MS = 1500;

/**
 * The stage markers the gateway's switch stream emits today, in
 * execution order, with their display labels (the same wording the
 * workshop's status bar uses). Unknown stages are appended as they
 * arrive, so a gateway that grows stages never breaks the overlay.
 */
const KNOWN_STAGES: ReadonlyArray<readonly [id: string, label: string]> = [
  ["loading-profile", "Loading profile"],
  ["stopping-models", "Stopping models"],
  ["starting-models", "Starting models"],
];

/** The overlay controller handed to the profile switcher. */
export interface ApplyOverlay {
  /** Mounts the overlay with the known stages listed as pending. */
  open(title: string): void;
  /** Marks `stage` active; the previously active stage becomes done. */
  beginStage(stage: string): void;
  /** Terminal success: marks every begun stage done and closes. */
  finish(): void;
  /** Terminal failure: marks the active stage failed, then closes. */
  fail(message: string): void;
}

/** Creates an overlay controller that mounts into `host` when opened. */
export function createApplyOverlay(host: HTMLElement): ApplyOverlay {
  let element: HTMLElement | null = null;
  let list: HTMLElement | null = null;
  let active: HTMLElement | null = null;
  let restoreFocus: HTMLElement | null = null;

  const close = () => {
    element?.remove();
    element = null;
    list = null;
    active = null;
    // Hand focus back to where it was when the overlay took it.
    if (restoreFocus?.isConnected) {
      restoreFocus.focus();
    }
    restoreFocus = null;
  };

  const setState = (row: HTMLElement, state: "active" | "done" | "failed") => {
    row.classList.remove("is-active", "is-done", "is-failed");
    row.classList.add(`is-${state}`);
    const icon = row.querySelector(".stage-icon");
    if (!icon) {
      return;
    }
    if (state === "active") {
      const spinner = document.createElement("span");
      spinner.className = "spinner";
      icon.replaceChildren(spinner, visuallyHidden("in progress"));
    } else if (state === "done") {
      icon.replaceChildren(iconSvg(Check), visuallyHidden("done"));
    } else {
      icon.replaceChildren(iconSvg(X), visuallyHidden("failed"));
    }
  };

  const stageRow = (stage: string, label: string): HTMLElement => {
    const row = document.createElement("li");
    row.className = "stage";
    row.dataset["stage"] = stage;
    const icon = document.createElement("span");
    icon.className = "stage-icon";
    const text = document.createElement("span");
    text.className = "stage-label";
    text.textContent = label;
    row.append(icon, text);
    return row;
  };

  return {
    open(title: string): void {
      close();
      element = document.createElement("div");
      element.className = "overlay apply-overlay";
      const card = document.createElement("section");
      card.className = "modal";
      // A non-dismissable progress dialog: it announces its stage
      // changes politely and holds focus while the switch runs, so
      // the keyboard never lands on the dimmed chrome behind it.
      card.setAttribute("role", "alertdialog");
      card.setAttribute("aria-modal", "true");
      card.setAttribute("aria-live", "polite");
      card.tabIndex = -1;
      const heading = document.createElement("h2");
      heading.id = "apply-overlay-title";
      heading.textContent = title;
      card.setAttribute("aria-labelledby", heading.id);
      list = document.createElement("ul");
      list.className = "stage-list";
      for (const [stage, label] of KNOWN_STAGES) {
        list.append(stageRow(stage, label));
      }
      card.append(heading, list);
      element.append(card);
      host.append(element);
      // Duck-typed: the HTMLElement global is absent under node --test.
      const focused = document.activeElement as HTMLElement | null;
      restoreFocus = focused && typeof focused.focus === "function" ? focused : null;
      card.focus();
    },

    beginStage(stage: string): void {
      if (!list) {
        return;
      }
      if (active) {
        setState(active, "done");
      }
      let row = list.querySelector<HTMLElement>(`[data-stage="${stage}"]`);
      if (!row) {
        row = stageRow(stage, stage);
        list.append(row);
      }
      setState(row, "active");
      active = row;
    },

    finish(): void {
      if (active) {
        setState(active, "done");
      }
      close();
    },

    fail(message: string): void {
      if (!element) {
        return;
      }
      if (active) {
        setState(active, "failed");
      }
      const note = document.createElement("p");
      note.className = "field-error";
      note.textContent = message;
      element.querySelector(".modal")?.append(note);
      scheduleTimeout(close, ERROR_HOLD_MS);
    },
  };
}

/** Renders a lucide icon as a decorative inline SVG. */
function iconSvg(icon: Parameters<typeof lucideElement>[0]): SVGElement {
  return lucideElement(icon, { "aria-hidden": "true", width: 16, height: 16 });
}

/** Screen-reader-only status text beside the visual stage icon. */
function visuallyHidden(text: string): HTMLElement {
  const span = document.createElement("span");
  span.className = "visually-hidden";
  span.textContent = text;
  return span;
}
