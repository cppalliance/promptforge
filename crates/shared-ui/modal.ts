// The focus-trapped modal dialog shared by both UIs: an overlay inside a
// host element, a role="dialog" (or "alertdialog") surface, an optional
// labeled text field, a Tab focus trap, Escape dismissal, optional
// backdrop dismissal, and focus return to the invoker. Merged from the
// gateway's confirm-modal and the workshop's editor-dialog so the two
// behaviors never diverge.
//
// Class contract: the overlay carries `modal-overlay` plus
// `${classPrefix}-overlay`; the dialog carries `modal-dialog` plus
// `${classPrefix}`; the title, message, field, label, input, actions, and
// buttons carry `${classPrefix}__title` / `__line` / `__field` / `__label`
// / `__input` / `__actions` / `__button` (with a `--danger` modifier).
// modal.css skins the base classes inside the components layer, so a
// consumer's own per-prefix rules always win.

import "./modal.css";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/** An optional single-line text field between the message and the actions. */
export interface ModalField {
  /** The input's id, unique per dialog kind for the label association. */
  readonly id: string;
  readonly label: string;
}

/**
 * One dialog action. `run` executes after the dialog dismisses, receiving
 * the field's trimmed value (the empty string when the dialog has none).
 */
export interface ModalButton {
  readonly label: string;
  /**
   * The button's full class list. Defaults to `${classPrefix}__button`,
   * with a `--danger` modifier when `danger` is set.
   */
  readonly className?: string;
  /** Style the button as destructive (only with the default className). */
  readonly danger?: boolean;
  /** Disables the button while the field's trimmed value is empty. */
  readonly requiresValue?: boolean;
  readonly run: (value: string) => void;
}

/** Construction options for {@link openModal}. */
export interface ModalOptions {
  /** The element the overlay mounts into. */
  readonly host: HTMLElement;
  /** BEM-style class prefix, e.g. "confirm" or "editor-close". */
  readonly classPrefix: string;
  /** The title element's id, unique per dialog kind for aria-labelledby. */
  readonly titleId: string;
  readonly title: string;
  readonly message: string;
  /** The dialog's role; defaults to "dialog". */
  readonly role?: "dialog" | "alertdialog";
  readonly field?: ModalField;
  readonly buttons: readonly ModalButton[];
  /** Dismiss when the pointer presses the dimmed backdrop. */
  readonly dismissOnBackdrop?: boolean;
  /** Called when Escape or the backdrop dismisses the dialog. */
  readonly onDismiss?: () => void;
}

/** The open dialog's handle. */
export interface ModalHandle {
  /** Dismisses the dialog if it is still open; safe to call twice. */
  close(): void;
  /** True once the dialog has dismissed. */
  readonly closed: boolean;
}

/**
 * Opens the dialog and focuses its field when it has one, its first
 * button otherwise. A second call while the same dialog kind is open in
 * the same host is a no-op and returns an already-closed handle. Escape
 * and backdrop dismissal return focus to the element that was focused
 * when the dialog opened.
 */
export function openModal(options: ModalOptions): ModalHandle {
  const prefix = options.classPrefix;
  if (options.host.querySelector(`.${prefix}-overlay`) !== null) {
    // The open dialog is owned by the call that created it.
    return { close: () => undefined, closed: true };
  }
  // Duck-typed: the HTMLElement global is absent under node --test.
  const active = document.activeElement as HTMLElement | null;
  const invoker = active && typeof active.focus === "function" ? active : null;

  const overlay = document.createElement("div");
  overlay.className = `modal-overlay ${prefix}-overlay`;

  const dialog = document.createElement("section");
  dialog.className = `modal-dialog ${prefix}`;
  dialog.setAttribute("role", options.role ?? "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-labelledby", options.titleId);

  const title = document.createElement("h2");
  title.id = options.titleId;
  title.className = `${prefix}__title`;
  title.textContent = options.title;

  const message = document.createElement("p");
  message.id = `${prefix}-message`;
  message.className = `${prefix}__line`;
  message.textContent = options.message;
  dialog.setAttribute("aria-describedby", message.id);

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
  actions.className = `modal-actions ${prefix}__actions`;

  let dismissed = false;
  const dismiss = (): void => {
    if (dismissed) {
      return;
    }
    dismissed = true;
    document.removeEventListener("keydown", onKeydown, true);
    overlay.remove();
    if (invoker?.isConnected) {
      invoker.focus();
    }
  };

  const buttons: HTMLButtonElement[] = [];
  const valueButtons: HTMLButtonElement[] = [];
  for (const def of options.buttons) {
    const button = document.createElement("button");
    button.type = "button";
    button.className =
      def.className ??
      (def.danger === true ? `${prefix}__button ${prefix}__button--danger` : `${prefix}__button`);
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
      options.onDismiss?.();
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
  if (options.dismissOnBackdrop === true) {
    overlay.addEventListener("click", (event) => {
      if (event.target === overlay) {
        dismiss();
        options.onDismiss?.();
      }
    });
  }
  options.host.appendChild(overlay);
  const firstFocus: HTMLElement | undefined = input ?? buttons[0];
  if (firstFocus) {
    firstFocus.focus();
  }
  return {
    close: dismiss,
    get closed() {
      return dismissed;
    },
  };
}
