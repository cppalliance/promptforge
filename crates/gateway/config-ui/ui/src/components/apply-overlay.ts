// Full-screen apply overlay [Adapted: Unsloth]: a dimmed layer centering
// a card that shows the gateway's live activity while an apply runs - a
// spinner beside the `Progress` text the `GET /admin/progress` stream
// carries ("Downloading qwen 45%", "Applying configuration"), fed
// through `observe` - a check once the apply passed, and an error mark
// when it died. The card carries a Cancel button (the overlay hides the
// status bar's own cancel control) that fires the caller's cancel hook
// once. The terminal event closes the overlay - instantly on success,
// after a short hold on failure so the failed state is seen (the toast
// carries the message onward).

import { Check, X, createElement as lucideElement } from "lucide";

import { scheduleTimeout } from "shared-ui/toast";

import type { Progress } from "../services/gateway-api";

/** How long a failed overlay stays up before removing itself. */
const ERROR_HOLD_MS = 1500;

/**
 * What the activity row says before the gateway reports anything: the
 * apply request is in flight but its command has not begun (it may be
 * queued behind the boot load) or the stream has not delivered yet.
 */
const WAITING_TEXT = "Waiting for the gateway";

/** The overlay controller handed to the composition root. */
export interface ApplyOverlay {
  /** Mounts the overlay with the activity row waiting. */
  open(title: string): void;
  /**
   * Feeds one `GET /admin/progress` snapshot. A busy snapshot puts its
   * text in the activity row; an idle one shows the waiting text, since
   * the apply the card covers has not settled yet.
   */
  observe(progress: Progress): void;
  /** Terminal success: marks the activity done and closes. */
  finish(): void;
  /** Terminal failure: marks the activity failed, then closes. */
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
  let row: HTMLElement | null = null;
  let label: HTMLElement | null = null;
  let cancel: HTMLButtonElement | null = null;
  let restoreFocus: HTMLElement | null = null;

  const close = () => {
    element?.remove();
    element = null;
    row = null;
    label = null;
    cancel = null;
    // Hand focus back to where it was when the overlay took it.
    if (restoreFocus?.isConnected) {
      restoreFocus.focus();
    }
    restoreFocus = null;
  };

  const setState = (state: "active" | "done" | "failed") => {
    if (!row) {
      return;
    }
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
      // A non-dismissable progress dialog: it announces its activity
      // changes politely and holds focus while the apply runs, so the
      // keyboard never lands on the dimmed chrome behind it.
      card.setAttribute("role", "alertdialog");
      card.setAttribute("aria-modal", "true");
      card.setAttribute("aria-live", "polite");
      card.tabIndex = -1;
      const heading = document.createElement("h2");
      heading.id = "apply-overlay-title";
      heading.textContent = title;
      card.setAttribute("aria-labelledby", heading.id);
      const list = document.createElement("ul");
      list.className = "stage-list";
      row = document.createElement("li");
      row.className = "stage";
      const icon = document.createElement("span");
      icon.className = "stage-icon";
      label = document.createElement("span");
      label.className = "stage-label";
      label.textContent = WAITING_TEXT;
      row.append(icon, label);
      list.append(row);
      setState("active");
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

    observe(progress: Progress): void {
      if (!label) {
        return;
      }
      // Updated in place: a flood of snapshots never re-creates nodes.
      label.textContent = progress.busy && progress.text !== "" ? progress.text : WAITING_TEXT;
    },

    finish(): void {
      setState("done");
      close();
    },

    fail(message: string): void {
      if (!element) {
        return;
      }
      setState("failed");
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
