// Themed modal dialogs for the workshop panels: an overlay inside the
// panel element, a role="dialog" surface, an optional labeled text field,
// a Tab focus trap, Escape dismissal, and focus return to the invoker.
// The editor's conflict and close prompts and the workshop tree's Add
// Folder prompt are built through this one helper so their behavior
// never diverges.

import { toDisposable, type IDisposable } from "../../base/lifecycle";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/**
 * One dialog action. `run` executes after the dialog dismisses, receiving
 * the field's trimmed value (the empty string when the dialog has none).
 */
export interface PanelDialogButton {
  readonly label: string;
  readonly danger?: boolean;
  /** Disables the button while the field's trimmed value is empty. */
  readonly requiresValue?: boolean;
  readonly run: (value: string) => void;
}

/** An optional single-line text field between the message and the actions. */
export interface PanelDialogField {
  /** The input's id, unique per dialog kind for the label association. */
  readonly id: string;
  readonly label: string;
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
  readonly field?: PanelDialogField;
  readonly buttons: readonly PanelDialogButton[];
}

/**
 * Opens the dialog and focuses its field when it has one, its first
 * button otherwise. A second call while the same dialog kind is open is
 * a no-op. Escape and Cancel-style dismissal return focus to the element
 * that was focused when the dialog opened.
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

  let input: HTMLInputElement | null = null;
  let field: HTMLDivElement | null = null;
  if (options.field) {
    field = document.createElement("div");
    field.className = `${prefix}__field`;
    const label = document.createElement("label");
    label.className = `${prefix}__label`;
    label.htmlFor = options.field.id;
    label.textContent = options.field.label;
    input = document.createElement("input");
    input.type = "text";
    input.id = options.field.id;
    input.className = `${prefix}__input`;
    field.append(label, input);
  }

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
  const valueButtons: HTMLButtonElement[] = [];
  for (const def of options.buttons) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = def.danger === true ? `${prefix}__button ${prefix}__button--danger` : `${prefix}__button`;
    button.textContent = def.label;
    if (def.requiresValue === true) {
      button.disabled = true;
      valueButtons.push(button);
    }
    button.addEventListener("click", () => {
      const value = input?.value.trim() ?? "";
      dismiss();
      def.run(value);
    });
    buttons.push(button);
    actions.appendChild(button);
  }

  if (input) {
    const boundInput = input;
    boundInput.addEventListener("input", () => {
      const empty = boundInput.value.trim() === "";
      for (const button of valueButtons) {
        button.disabled = empty;
      }
    });
    // Enter submits through the first value-gated button, which stays
    // disabled (and therefore inert) while the field is empty.
    boundInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        const primary = valueButtons[0] ?? buttons[0];
        if (primary && !primary.disabled) {
          primary.click();
        }
      }
    });
  }

  if (field) {
    dialog.append(title, message, field, actions);
  } else {
    dialog.append(title, message, actions);
  }
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
  const firstFocus: HTMLElement | undefined = input ?? buttons[0];
  if (firstFocus) {
    firstFocus.focus();
  }
  return toDisposable(dismiss);
}
