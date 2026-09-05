// Full-screen switch/apply overlay [Adapted: Unsloth]: a dimmed layer
// centering a card that lists the gateway's switch stages, each with a
// spinner while active, a check when passed, and an error mark when the
// switch dies in it. Stages come from the `GET /admin/progress` hub
// stream through `observe`: a `Begun` leaf whose label is a known stage
// opens that stage. While a stage runs, a `<stage>/<model>/download`
// leaf drives a detail row under the active stage - "Downloading
// <model>" with the shared inline progress bar set by the leaf's
// `Updated` fractions, "Verifying <model>" once the download leaf
// finishes, "Starting <model>" when the `<model>/ready` leaf begins -
// cleared when the stage ends. The card carries a Cancel button (the
// overlay hides the status bar's own cancel control) that fires the
// caller's cancel hook once. The terminal event closes the overlay -
// instantly on success, after a short hold on failure so the failed
// stage is seen (the toast carries the message onward).

import { Check, X, createElement as lucideElement } from "lucide";

import { createProgressBar, type ProgressBar } from "shared-ui/progress";
import { scheduleTimeout } from "shared-ui/toast";

import { isRecord } from "../services/json";

/** How long a failed overlay stays up before removing itself. */
const ERROR_HOLD_MS = 1500;

/**
 * The stage leaves the gateway's switch registers on its progress tree,
 * in execution order, with their display labels (the same wording the
 * workshop's status bar uses). The gateway skips the download when the
 * profile names no local model and the stop when nothing old runs, and
 * runs the download after the stop-free cut-over on a cold boot, so a
 * switch may light a subset of these rows, out of this order. Unknown
 * stages begun through `beginStage` are appended as they arrive, so a
 * gateway that grows stages never breaks the overlay; `observe` only
 * maps these four.
 */
const KNOWN_STAGES: ReadonlyArray<readonly [id: string, label: string]> = [
  ["loading-profile", "Loading profile"],
  ["downloading-models", "Downloading models"],
  ["stopping-models", "Stopping models"],
  ["starting-models", "Starting models"],
];

/** The overlay controller handed to the composition root. */
export interface ApplyOverlay {
  /** Mounts the overlay with the known stages listed as pending. */
  open(title: string): void;
  /** Marks `stage` active; the previously active stage becomes done. */
  beginStage(stage: string): void;
  /**
   * Feeds one raw `GET /admin/progress` event. A hub `ProgressEvent`
   * whose `state` is `Begun` and whose `label` is a known stage begins
   * that stage. A `Begun` whose `path` is `<stage>/<model>/download`
   * opens the detail row under the active stage, the leaf's `Updated`
   * fractions set its bar, the leaf's `Finished` flips the row to the
   * verifying label, and a `<stage>/<model>/ready` `Begun` flips it to
   * the starting label. Every other shape is ignored.
   */
  observe(event: unknown): void;
  /** Terminal success: marks every begun stage done and closes. */
  finish(): void;
  /** Terminal failure: marks the active stage failed, then closes. */
  fail(message: string): void;
}

/** Construction options for {@link createApplyOverlay}. */
export interface ApplyOverlayOptions {
  /**
   * Runs when the card's Cancel button is clicked, once per opening.
   * The hook owns its own error reporting; the overlay stays up until
   * the operation it covers settles through `finish` or `fail`.
   */
  onCancel?: () => void | Promise<void>;
}

/** Creates an overlay controller that mounts into `host` when opened. */
export function createApplyOverlay(
  host: HTMLElement,
  options: ApplyOverlayOptions = {},
): ApplyOverlay {
  let element: HTMLElement | null = null;
  let list: HTMLElement | null = null;
  let active: HTMLElement | null = null;
  let cancel: HTMLButtonElement | null = null;
  let restoreFocus: HTMLElement | null = null;
  let detail: {
    row: HTMLElement;
    text: HTMLElement;
    bar: ProgressBar;
    percent: HTMLElement;
  } | null = null;
  let downloadPath: string | null = null;

  const clearDetail = () => {
    detail?.row.remove();
    detail = null;
    downloadPath = null;
  };

  const close = () => {
    element?.remove();
    element = null;
    list = null;
    active = null;
    cancel = null;
    clearDetail();
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

  const beginStage = (stage: string): void => {
    if (!list) {
      return;
    }
    if (active) {
      setState(active, "done");
      // A stage change retires the download/verify/start detail row.
      clearDetail();
    }
    let row = list.querySelector<HTMLElement>(`[data-stage="${stage}"]`);
    if (!row) {
      row = stageRow(stage, stage);
      list.append(row);
    }
    setState(row, "active");
    active = row;
  };

  /**
   * Shows the detail row under the active stage with `label`, creating
   * it on first use and updating it in place afterwards, so a flood of
   * coalesced `Updated` frames never re-creates nodes.
   */
  const showDetail = (label: string): void => {
    if (!active) {
      return;
    }
    if (!detail) {
      const row = document.createElement("li");
      row.className = "stage-detail";
      const text = document.createElement("span");
      text.className = "stage-detail-label";
      const bar = createProgressBar("Model download progress");
      const percent = document.createElement("span");
      percent.className = "stage-detail-percent";
      row.append(text, bar.element, percent);
      detail = { row, text, bar, percent };
    }
    detail.text.textContent = label;
    active.after(detail.row);
  };

  /** Handles a `Begun` frame for a non-stage leaf of a known stage. */
  const observeLeafBegun = (path: string): void => {
    const leaf = splitLeafPath(path);
    if (!leaf || !KNOWN_STAGES.some(([id]) => id === leaf.stage)) {
      return;
    }
    if (leaf.leaf === "download") {
      // The most recent download leaf wins: a switch that stages
      // several models shows one row at a time.
      downloadPath = path;
      showDetail(`Downloading ${leaf.model}`);
      if (detail) {
        detail.bar.setFraction(0);
        detail.percent.textContent = "0%";
      }
    } else if (leaf.leaf === "ready") {
      downloadPath = null;
      showDetail(`Starting ${leaf.model}`);
      if (detail) {
        detail.bar.setFraction(null);
        detail.percent.textContent = "";
      }
    }
  };

  const cancelButton = (onCancel: () => void | Promise<void>): HTMLButtonElement => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "button button-outline apply-overlay-cancel";
    button.textContent = "Cancel";
    button.addEventListener("click", () => {
      // One request per opening: the command settles as cancelled at its
      // next boundary, and the covered operation's own failure closes
      // the overlay.
      button.disabled = true;
      void onCancel();
    });
    return button;
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
      if (options.onCancel) {
        const actions = document.createElement("div");
        actions.className = "modal-actions";
        cancel = cancelButton(options.onCancel);
        actions.append(cancel);
        card.append(actions);
      }
      element.append(card);
      host.append(element);
      // Duck-typed: the HTMLElement global is absent under node --test.
      const focused = document.activeElement as HTMLElement | null;
      restoreFocus = focused && typeof focused.focus === "function" ? focused : null;
      card.focus();
    },

    beginStage,

    observe(event: unknown): void {
      // serde's externally tagged `EventState`: `{"Begun":{"weight":..}}`.
      if (!isRecord(event) || !isRecord(event["state"])) {
        return;
      }
      const state = event["state"];
      if ("Begun" in state) {
        const label = event["label"];
        if (typeof label === "string" && KNOWN_STAGES.some(([id]) => id === label)) {
          beginStage(label);
          return;
        }
        if (typeof event["path"] === "string") {
          observeLeafBegun(event["path"]);
        }
        return;
      }
      // `Updated` and `Finished` move only the tracked download leaf's
      // row; frames for any other path change nothing.
      const path = event["path"];
      if (typeof path !== "string" || path !== downloadPath || !detail) {
        return;
      }
      if ("Updated" in state) {
        const updated = state["Updated"];
        const fraction = isRecord(updated) ? updated["fraction"] : null;
        if (typeof fraction !== "number") {
          return;
        }
        const clamped = Math.min(Math.max(fraction, 0), 1);
        detail.bar.setFraction(clamped);
        detail.percent.textContent = `${Math.round(clamped * 100)}%`;
      } else if ("Finished" in state) {
        // The verify leaf runs next; the `ready` leaf's `Begun` flips
        // the row to the starting label.
        downloadPath = null;
        const model = splitLeafPath(path)?.model;
        if (model) {
          detail.text.textContent = `Verifying ${model}`;
        }
        detail.bar.setFraction(null);
        detail.percent.textContent = "";
      }
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
      // The operation is over; a cancel during the hold has no target.
      if (cancel) {
        cancel.disabled = true;
      }
      const note = document.createElement("p");
      note.className = "field-error";
      note.textContent = message;
      element.querySelector(".modal")?.append(note);
      scheduleTimeout(close, ERROR_HOLD_MS);
    },
  };
}

/**
 * Splits a hub leaf path such as `downloading-models/glm-4-9b/download`
 * into its stage prefix, model name, and leaf name: the stage is the
 * first segment, the leaf the last, and the model everything between,
 * so a model name that itself contains a slash (the config validates
 * only that it is non-empty) still displays in full. Returns null for
 * anything shallower, so unknown path shapes are ignored.
 */
function splitLeafPath(path: string): { stage: string; model: string; leaf: string } | null {
  const segments = path.split("/");
  if (segments.length < 3) {
    return null;
  }
  const stage = segments[0] ?? "";
  const model = segments.slice(1, -1).join("/");
  const leaf = segments[segments.length - 1] ?? "";
  if (!stage || !model || !leaf) {
    return null;
  }
  return { stage, model, leaf };
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
