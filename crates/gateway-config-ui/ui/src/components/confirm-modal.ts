// Confirm dialog [Adapted: llama.cpp]: the shared focus-trapped modal
// (shared-ui/modal) as a Cancel/confirm pair. Focus moves into the
// dialog (landing on Cancel, the safe default), Escape and the backdrop
// cancel, and focus returns to the opener when the dialog closes.

import { openModal } from "shared-ui/modal";

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
    let settled = false;
    const settle = (choice: boolean): void => {
      if (!settled) {
        settled = true;
        resolve(choice);
      }
    };
    const handle = openModal({
      host,
      classPrefix: "confirm",
      titleId: "confirm-title",
      title: options.title,
      message: options.body,
      role: "alertdialog",
      dismissOnBackdrop: true,
      onDismiss: () => settle(false),
      buttons: [
        { label: "Cancel", className: "button button-outline", run: () => settle(false) },
        {
          label: options.confirmLabel,
          className: options.danger ? "button button-danger" : "button button-primary",
          run: () => settle(true),
        },
      ],
    });
    // The duplicate guard no-ops a second dialog of the same kind; the
    // caller's promise still settles, as a cancellation.
    if (handle.closed) {
      settle(false);
    }
  });
}
