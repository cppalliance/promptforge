// Bottom-right toast stack [Adapted: Open WebUI]: success/error/info
// entries that dismiss themselves after four seconds. Shared by both
// UIs: the gateway's composition root mounts one for shell and view
// notifications, the workshop mounts one for update notifications.

import "./toast.css";

/** How long a toast stays before dismissing itself. */
const TOAST_LIFETIME_MS = 4000;

/** The toast severity, mapping to the accent-border variants. */
export type ToastKind = "success" | "error" | "info";

/** The toast stack: one fixed-position element plus a show method. */
export interface ToastStack {
  /** The stack element; the composition root appends it once. */
  element: HTMLElement;
  /** Pushes one toast; it removes itself after its lifetime. */
  show(message: string, kind: ToastKind): void;
}

/**
 * Schedules a callback without keeping a Node test process alive:
 * Node's setTimeout returns a handle with `unref`, the browser's
 * returns a number and the optional call is a no-op.
 */
export function scheduleTimeout(callback: () => void, ms: number): void {
  const timer = setTimeout(callback, ms);
  (timer as unknown as { unref?: () => void }).unref?.();
}

/** Creates the toast stack. */
export function createToastStack(): ToastStack {
  const element = document.createElement("div");
  element.className = "toast-stack";
  // A live region: screen readers announce each appended toast.
  element.setAttribute("role", "status");
  element.setAttribute("aria-live", "polite");

  return {
    element,
    show(message: string, kind: ToastKind): void {
      const toast = document.createElement("div");
      toast.className = `toast toast-${kind}`;
      toast.textContent = message;
      element.append(toast);
      scheduleTimeout(() => toast.remove(), TOAST_LIFETIME_MS);
    },
  };
}
