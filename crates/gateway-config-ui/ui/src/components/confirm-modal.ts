// Confirm dialog [Adapted: llama.cpp]: a dimmed overlay centering a
// card with a title, a body naming the target, and a Cancel/confirm
// pair. Focus moves into the dialog (landing on Cancel, the safe
// default), Escape and the backdrop cancel, and focus returns to the
// opener when the dialog closes.

/** Construction options for {@link confirmDialog}. */
export interface ConfirmOptions {
  /** The dialog heading. */
  title: string;
  /** The body sentence naming the target (and its size, for files). */
  body: string;
  /** The confirming button's label. */
  confirmLabel: string;
  /** Style the confirming button as destructive. */
  danger?: boolean;
}

/**
 * Opens the confirm dialog in `host` and resolves with the choice:
 * true for the confirming action, false for Cancel/Escape/backdrop.
 */
export function confirmDialog(host: HTMLElement, options: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "overlay confirm-overlay";

    const card = document.createElement("section");
    card.className = "modal";
    card.setAttribute("role", "alertdialog");
    card.setAttribute("aria-modal", "true");

    const heading = document.createElement("h2");
    heading.id = "confirm-title";
    heading.textContent = options.title;
    card.setAttribute("aria-labelledby", heading.id);

    const body = document.createElement("p");
    body.id = "confirm-body";
    body.textContent = options.body;
    card.setAttribute("aria-describedby", body.id);

    const actions = document.createElement("div");
    actions.className = "modal-actions";

    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "button button-outline";
    cancel.textContent = "Cancel";

    const confirm = document.createElement("button");
    confirm.type = "button";
    confirm.className = options.danger ? "button button-danger" : "button button-primary";
    confirm.textContent = options.confirmLabel;

    actions.append(cancel, confirm);
    card.append(heading, body, actions);
    overlay.append(card);
    host.append(overlay);

    // Duck-typed: the HTMLElement global is absent under node --test.
    const opener = document.activeElement as HTMLElement | null;
    const restore = opener && typeof opener.focus === "function" ? opener : null;

    const close = (choice: boolean): void => {
      overlay.remove();
      if (restore?.isConnected) {
        restore.focus();
      }
      resolve(choice);
    };

    cancel.addEventListener("click", () => close(false));
    confirm.addEventListener("click", () => close(true));
    overlay.addEventListener("click", (event) => {
      if (event.target === overlay) {
        close(false);
      }
    });
    card.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        close(false);
        return;
      }
      // A two-button trap: Tab cycles between Cancel and the action.
      if (event.key === "Tab") {
        event.preventDefault();
        const next = document.activeElement === cancel ? confirm : cancel;
        next.focus();
      }
    });

    cancel.focus();
  });
}
