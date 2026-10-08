// The focus-trapped modal dialog for the Workshop family's UIs: an overlay
// inside a container element, a role="dialog" (or "alertdialog") surface, an
// optional labeled text field, a Tab focus trap, Escape dismissal,
// optional backdrop dismissal, and focus return to the invoker.
//
// Class contract: the overlay has `modal-overlay` plus
// `${classPrefix}-overlay`; the dialog has `modal-dialog` plus
// `${classPrefix}`; the title, message, field, label, input, actions, and
// buttons have `${classPrefix}__title` / `__line` / `__field` / `__label`
// / `__input` / `__actions` / `__button` (with `--danger` and `--primary`
// modifiers). modal.css skins the base classes inside the components layer,
// so a consumer's own per-prefix rules always win.
//
// Skins: `skin: "confirmation"` is Cursor's save/revert/overwrite prompt and
// `skin: "form"` is its form modal (Add Folder, Choose Prompt). A skinned
// dialog adds `modal-overlay--${skin}` and `modal-dialog--${skin}`, styles its
// buttons with the controls.css classes (primary, danger, or secondary), and
// orders them the way the skin does: a confirmation puts the primary button
// first (Windows order: Save, Don't Save, Cancel), a form puts it last
// (Cancel first). The other buttons keep the order they were given.

import "./modal.css";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/** An optional single-line text field between the message and the actions. */
export interface ModalField {
  /** The input's id, unique per dialog kind for the label association. */
  readonly id: string;
  readonly label: string;
  /** Placeholder text shown while the field is empty. */
  readonly placeholder?: string;
  /** The field's initial text, selected on open so typing replaces it. */
  readonly value?: string;
}

/**
 * One dialog action. `run` executes after the dialog dismisses, receiving
 * the field's trimmed value (the empty string when the dialog has none).
 */
export interface ModalButton {
  readonly label: string;
  /**
   * The button's full class list. Defaults to `${classPrefix}__button`,
   * with `--danger` and `--primary` modifiers when those flags are set,
   * and the controls.css button classes when the dialog has a skin.
   */
  readonly className?: string;
  /** Style the button as destructive (only with the default className). */
  readonly danger?: boolean;
  /**
   * The dialog's main action (only with the default className). It takes
   * the first focus when the dialog has no field, and a skin places it:
   * first in a confirmation, last in a form.
   */
  readonly primary?: boolean;
  /** Disables the button while the field's trimmed value is empty. */
  readonly requiresValue?: boolean;
  readonly run: (value: string) => void;
}

/** The two dialog skins: Cursor's confirmation prompt and its form modal. */
export type ModalSkin = "confirmation" | "form";

/** Construction options for {@link openModal}. */
export interface ModalOptions {
  /** The element the overlay mounts into. */
  readonly container: HTMLElement;
  /** BEM-style class prefix, e.g. "confirm" or "editor-close". */
  readonly classPrefix: string;
  /** The title element's id, unique per dialog kind for aria-labelledby. */
  readonly titleId: string;
  readonly title: string;
  readonly message: string;
  /** The dialog's role; defaults to "dialog". */
  readonly role?: "dialog" | "alertdialog";
  /** Cursor's confirmation or form skin; without one the dialog is unskinned. */
  readonly skin?: ModalSkin;
  readonly field?: ModalField;
  readonly buttons: readonly ModalButton[];
  /** Dismiss when the pointer presses the dimmed backdrop. */
  readonly dismissOnBackdrop?: boolean;
  /** Called when Escape or the backdrop dismisses the dialog. */
  readonly onDismiss?: () => void;
}

/**
 * The buttons in the order the skin shows them: a confirmation puts the
 * primary buttons first, a form puts them last, and an unskinned dialog
 * keeps the order it was given. The rest keep their relative order.
 */
function orderedButtons(buttons: readonly ModalButton[], skin: ModalSkin | undefined): readonly ModalButton[] {
  if (skin === undefined) {
    return buttons;
  }
  const primary = buttons.filter((button) => button.primary === true);
  const rest = buttons.filter((button) => button.primary !== true);
  return skin === "confirmation" ? [...primary, ...rest] : [...rest, ...primary];
}

/** A button's default class list: the prefix classes, plus the controls.css ones in a skinned dialog. */
function buttonClassName(prefix: string, def: ModalButton, skin: ModalSkin | undefined): string {
  const classes = [`${prefix}__button`];
  if (def.danger === true) {
    classes.push(`${prefix}__button--danger`);
  }
  if (def.primary === true) {
    classes.push(`${prefix}__button--primary`);
  }
  if (skin !== undefined) {
    const variant = def.primary === true ? "button-primary" : def.danger === true ? "button-danger" : "button-secondary";
    classes.push("button", variant);
  }
  return classes.join(" ");
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
 * the same container element is a no-op and returns an already-closed
 * handle. Escape and backdrop dismissal return focus to the element that
 * was focused when the dialog opened.
 */
export function openModal(options: ModalOptions): ModalHandle {
  const prefix = options.classPrefix;
  if (options.container.querySelector(`.${prefix}-overlay`) !== null) {
    // The open dialog is owned by the call that created it.
    return { close: () => undefined, closed: true };
  }
  // Duck-typed: the HTMLElement global is absent under node --test.
  const active = document.activeElement as HTMLElement | null;
  const invoker = active && typeof active.focus === "function" ? active : null;

  const skin = options.skin;
  const overlay = document.createElement("div");
  overlay.className = `modal-overlay ${prefix}-overlay`;
  if (skin !== undefined) {
    overlay.classList.add(`modal-overlay--${skin}`);
  }

  const dialog = document.createElement("section");
  dialog.className = `modal-dialog ${prefix}`;
  if (skin !== undefined) {
    dialog.classList.add(`modal-dialog--${skin}`);
  }
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
    input.className = skin === undefined ? `${prefix}__input` : `${prefix}__input input`;
    if (options.field.placeholder !== undefined) {
      input.placeholder = options.field.placeholder;
    }
    if (options.field.value !== undefined) {
      input.value = options.field.value;
    }
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
  let primaryButton: HTMLButtonElement | null = null;
  for (const def of orderedButtons(options.buttons, skin)) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = def.className ?? buttonClassName(prefix, def, skin);
    button.textContent = def.label;
    if (def.primary === true && primaryButton === null) {
      primaryButton = button;
    }
    if (def.requiresValue === true) {
      // Disabled until the field holds text; an initial value counts.
      button.disabled = (options.field?.value ?? "").trim() === "";
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
    // disabled (and therefore inert) while the field is empty; without
    // one, through the primary button, else the first.
    boundInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        const submit = valueButtons[0] ?? primaryButton ?? buttons[0];
        if (submit && !submit.disabled) {
          submit.click();
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
  options.container.appendChild(overlay);
  const firstFocus: HTMLElement | undefined = input ?? primaryButton ?? buttons[0];
  if (firstFocus) {
    firstFocus.focus();
  }
  if (input && options.field?.value !== undefined) {
    input.select();
  }
  return {
    close: dismiss,
    get closed() {
      return dismissed;
    },
  };
}
