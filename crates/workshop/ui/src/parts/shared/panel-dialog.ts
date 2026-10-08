// Themed modal dialogs for the workshop panels, built on the shared
// focus-trapped modal (@workshop/look/modal): an overlay inside the panel
// element, a role="dialog" surface, an optional labeled text field, a
// Tab focus trap, Escape dismissal, and focus return to the invoker.
// The editor's conflict, revert, and close prompts and the workshop tree's
// Add Folder prompt are built through this one helper so their behavior
// never diverges. The `skin` picks Cursor's confirmation prompt (save,
// revert, overwrite) or its form modal (Add Folder); the skin owns the
// dialog's look, so a caller carries no per-dialog stylesheet.

import { openModal, type ModalSkin } from "@workshop/look/modal";

import { toDisposable, type IDisposable } from "@workshop/platform/lifecycle";

/**
 * One dialog action. `run` executes after the dialog dismisses, receiving
 * the field's trimmed value (the empty string when the dialog has none).
 */
export interface PanelDialogButton {
  readonly label: string;
  readonly danger?: boolean;
  /** The dialog's main action: it takes first focus, and the skin places it. */
  readonly primary?: boolean;
  /** Disables the button while the field's trimmed value is empty. */
  readonly requiresValue?: boolean;
  readonly run: (value: string) => void;
}

/** An optional single-line text field between the message and the actions. */
export interface PanelDialogField {
  /** The input's id, unique per dialog kind for the label association. */
  readonly id: string;
  readonly label: string;
  /** Placeholder text shown while the field is empty. */
  readonly placeholder?: string;
  /** The field's initial text, selected on open so typing replaces it. */
  readonly value?: string;
}

export interface PanelDialogOptions {
  /** The panel element the overlay mounts into. */
  readonly container: HTMLElement;
  /** BEM-style class prefix, e.g. "ws-editor-conflict" or "ws-editor-close". */
  readonly classPrefix: string;
  /** The title element's id, unique per dialog kind for aria-labelledby. */
  readonly titleId: string;
  readonly title: string;
  readonly message: string;
  /** Cursor's confirmation or form skin; without one the dialog is unskinned. */
  readonly skin?: ModalSkin;
  readonly field?: PanelDialogField;
  readonly buttons: readonly PanelDialogButton[];
  /** Called when Escape dismisses the dialog; no button's run fires then. */
  readonly onDismiss?: () => void;
}

/**
 * Opens the dialog and focuses its field when it has one, its first
 * button otherwise. A second call while the same dialog kind is open is
 * a no-op. Escape dismissal returns focus to the element that was
 * focused when the dialog opened.
 *
 * Returns a disposable that dismisses the dialog if it is still open, so
 * the invoking panel owns the document-level focus trap: a panel disposed
 * while its dialog is up tears the trap down with it.
 */
export function showPanelDialog(options: PanelDialogOptions): IDisposable {
  const handle = openModal(options);
  return toDisposable(() => handle.close());
}
