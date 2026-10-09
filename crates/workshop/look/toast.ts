// The toast stack, bottom-left, in Cursor's notification-toast style
// [Adapted: Open WebUI]: a card per message with a severity codicon, the
// newest on top and three at most, each leaving after its severity's
// lifetime. Hovering or focusing a toast pauses its timer and shows a
// close X. For the Workshop family's UIs: the workshop mounts one for
// update notifications, copy confirmations, and command failures.

import "./toast.css";
import { ICON_CHECK, ICON_CLOSE, ICON_ERROR, ICON_INFO, ICON_WARNING } from "./icons";

/** How long a toast stays, by severity: info and success 15s, warning 18s, error 20s. */
const TOAST_LIFETIME_MS = {
  info: 15000,
  success: 15000,
  warning: 18000,
  error: 20000,
} as const;

/** The most toasts the stack shows at once; a newer one pushes the oldest out. */
const MAX_VISIBLE = 3;

/** The toast severity, which picks the glyph, its color, and the lifetime. */
export type ToastKind = keyof typeof TOAST_LIFETIME_MS;

/** Each severity's codicon: a check for success, the severity glyphs for the rest. */
const KIND_ICONS: Readonly<Record<ToastKind, string>> = {
  info: ICON_INFO,
  success: ICON_CHECK,
  warning: ICON_WARNING,
  error: ICON_ERROR,
};

/** The toast stack: one fixed-position element plus a show method. */
export interface ToastStack {
  /** The stack element; the composition root appends it once. */
  element: HTMLElement;
  /** Pushes one toast on top; it removes itself after its lifetime. */
  show(message: string, kind: ToastKind): void;
}

/**
 * Schedules a callback without keeping a Node test process alive:
 * Node's setTimeout returns a handle with `unref`, the browser's
 * returns a number and the optional call is a no-op. Answers the
 * function that cancels the callback.
 */
export function scheduleTimeout(callback: () => void, ms: number): () => void {
  const timer = setTimeout(callback, ms);
  (timer as unknown as { unref?: () => void }).unref?.();
  return () => clearTimeout(timer);
}

/** One toast on the stack and the handle that removes it early. */
interface LiveToast {
  readonly element: HTMLElement;
  /** Removes the toast and cancels its timer. Safe to call twice. */
  dismiss(): void;
}

/** Creates the toast stack. */
export function createToastStack(): ToastStack {
  const element = document.createElement("div");
  element.className = "toast-stack";
  // A live region: screen readers announce each appended toast.
  element.setAttribute("role", "status");
  element.setAttribute("aria-live", "polite");

  // Newest first, matching the DOM order top to bottom.
  const live: LiveToast[] = [];

  function build(message: string, kind: ToastKind): LiveToast {
    const toast = document.createElement("div");
    toast.className = `toast toast-${kind}`;

    const icon = document.createElement("span");
    icon.className = "toast__icon";
    icon.setAttribute("aria-hidden", "true");
    icon.innerHTML = KIND_ICONS[kind];

    const text = document.createElement("span");
    text.className = "toast__message";
    text.textContent = message;

    const close = document.createElement("button");
    close.type = "button";
    close.className = "toast__close";
    close.setAttribute("aria-label", "Clear Notification");
    close.innerHTML = ICON_CLOSE;

    toast.append(icon, text, close);

    // The timer runs while nothing holds the toast. Hover and focus each
    // hold it: a hold cancels the timer and keeps the unspent time, and
    // the last release restarts the timer with what was left.
    let remaining: number = TOAST_LIFETIME_MS[kind];
    let startedAt = Date.now();
    let cancelTimer: (() => void) | null = null;
    let holds = 0;
    let dismissed = false;

    const dismiss = (): void => {
      if (dismissed) {
        return;
      }
      dismissed = true;
      cancelTimer?.();
      cancelTimer = null;
      toast.remove();
      const index = live.findIndex((entry) => entry.element === toast);
      if (index !== -1) {
        live.splice(index, 1);
      }
    };
    const start = (): void => {
      startedAt = Date.now();
      cancelTimer = scheduleTimeout(dismiss, remaining);
    };
    const hold = (): void => {
      holds += 1;
      if (holds === 1 && cancelTimer !== null) {
        cancelTimer();
        cancelTimer = null;
        remaining = Math.max(0, remaining - (Date.now() - startedAt));
      }
    };
    const release = (): void => {
      holds = Math.max(0, holds - 1);
      if (holds === 0 && !dismissed && cancelTimer === null) {
        start();
      }
    };

    toast.addEventListener("mouseenter", hold);
    toast.addEventListener("mouseleave", release);
    toast.addEventListener("focusin", hold);
    toast.addEventListener("focusout", release);
    close.addEventListener("click", dismiss);
    start();

    return { element: toast, dismiss };
  }

  return {
    element,
    show(message: string, kind: ToastKind): void {
      const toast = build(message, kind);
      live.unshift(toast);
      element.prepend(toast.element);
      // The oldest sit at the end; push out whatever no longer fits.
      while (live.length > MAX_VISIBLE) {
        live[live.length - 1]?.dismiss();
      }
    },
  };
}
