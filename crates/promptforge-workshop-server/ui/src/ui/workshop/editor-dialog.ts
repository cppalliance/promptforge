// Themed modal dialogs for the editor panel: an overlay inside the panel
// element, a role="dialog" surface, a Tab focus trap, Escape dismissal,
// and focus return to the invoker. Both editor dialogs - the
// modified-time conflict and the unsaved-changes close prompt - are built
// through this one helper so their behavior never diverges.

import { toDisposable, type IDisposable } from "../../base/lifecycle";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/** One dialog action. `run` executes after the dialog dismisses. */
export interface PanelDialogButton {
  readonly label: string;
  readonly danger?: boolean;
  readonly run: () => void;
}

export interface PanelDialogOptions {
  /** The panel element the overlay mounts into. */
  readonly host: HTMLElement;
  /** BEM-style class prefix, e.g. "editor-conflict" or "editor-close". */
  readonly classPrefix: string;
  /** The title element's id, unique per dialog kind for aria-labelledby. */
  readonly titleId: string;
  readonly title: string;
  readonly message: string;
  readonly buttons: readonly PanelDialogButton[];
}

/**
 * Opens the dialog and focuses its first button. A second call while the
 * same dialog kind is open is a no-op. Escape and Cancel-style dismissal
 * return focus to the element that was focused when the dialog opened.
 *
 * Returns a disposable that dismisses the dialog if it is still open, so
 * the invoking panel owns the document-level focus trap: a panel disposed
 * while its dialog is up tears the trap down with it.
 */
export function showPanelDialog(options: PanelDialogOptions): IDisposable {
  const prefix = options.classPrefix;
  if (options.host.querySelector(`.${prefix}-overlay`) !== null) {
    // The open dialog is owned by the call that created it.
    return toDisposable(() => undefined);
  }
  const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  const overlay = document.createElement("div");
  overlay.className = `${prefix}-overlay`;

  const dialog = document.createElement("section");
  dialog.className = prefix;
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-labelledby", options.titleId);

  const title = document.createElement("h2");
  title.id = options.titleId;
  title.className = `${prefix}__title`;
  title.textContent = options.title;

  const message = document.createElement("p");
  message.className = `${prefix}__line`;
  message.textContent = options.message;

  const actions = document.createElement("div");
  actions.className = `${prefix}__actions`;

  let dismissed = false;
  const dismiss = (): void => {
    if (dismissed) {
      return;
    }
    dismissed = true;
    document.removeEventListener("keydown", onKeydown, true);
    overlay.remove();
    invoker?.focus();
  };

  const buttons: HTMLButtonElement[] = [];
  for (const def of options.buttons) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = def.danger === true ? `${prefix}__button ${prefix}__button--danger` : `${prefix}__button`;
    button.textContent = def.label;
    button.addEventListener("click", () => {
      dismiss();
      def.run();
    });
    buttons.push(button);
    actions.appendChild(button);
  }

  dialog.append(title, message, actions);
  overlay.appendChild(dialog);

  const onKeydown = (event: KeyboardEvent): void => {
    if (event.key === "Escape") {
      event.preventDefault();
      dismiss();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const focusable = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)];
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) {
      event.preventDefault();
      return;
    }
    const active = document.activeElement;
    const outside = !active || !dialog.contains(active);
    if (event.shiftKey && (outside || active === first)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (outside || active === last)) {
      event.preventDefault();
      first.focus();
    }
  };

  document.addEventListener("keydown", onKeydown, true);
  options.host.appendChild(overlay);
  const firstButton = buttons[0];
  if (firstButton) {
    firstButton.focus();
  }
  return toDisposable(dismiss);
}
