// The Help menu's About dialog: a small themed modal naming the product,
// the application version, and the license. Focus is trapped inside the
// dialog while it is open, Escape and the Close button dismiss it, and
// focus returns to the element that opened it.

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";

// Mirrors [workspace.package] version in the workspace Cargo.toml; bump
// together with the crate version.
const APP_VERSION = "0.1.0";
const LICENSE = "BSL-1.0";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/**
 * Opens the About modal; a no-op while one is already open. Returns the
 * disposable that dismisses the dialog - Escape and the Close button
 * dispose it too.
 */
export function showAboutDialog(): IDisposable {
  if (document.querySelector(".about-dialog")) {
    // The open dialog owns its own teardown; there is nothing to release.
    return toDisposable(() => {});
  }
  const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  const overlay = document.createElement("div");
  overlay.className = "about-dialog-overlay";

  const dialog = document.createElement("section");
  dialog.className = "about-dialog";
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-labelledby", "about-dialog-title");

  const title = document.createElement("h2");
  title.id = "about-dialog-title";
  title.className = "about-dialog__title";
  title.textContent = "PromptForge";

  const version = document.createElement("p");
  version.className = "about-dialog__line";
  version.textContent = `Version ${APP_VERSION}`;

  const license = document.createElement("p");
  license.className = "about-dialog__line";
  license.textContent = `License: ${LICENSE}`;

  const close = document.createElement("button");
  close.type = "button";
  close.className = "about-dialog__close";
  close.textContent = "Close";

  dialog.append(title, version, license, close);
  overlay.appendChild(dialog);

  const store = new DisposableStore();

  function dismiss(): void {
    store.dispose();
  }

  function onKeydown(event: KeyboardEvent): void {
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
  }

  // Teardown order matters and mirrors the old dismiss(): the trap
  // listener detaches first, then the overlay leaves the DOM and focus
  // returns to the invoker.
  store.add(toDisposable(() => document.removeEventListener("keydown", onKeydown, true)));
  store.add(
    toDisposable(() => {
      overlay.remove();
      invoker?.focus();
    }),
  );

  // The Close button's listener is element-owned: it goes away with the
  // overlay and needs no registration.
  close.addEventListener("click", dismiss);
  document.addEventListener("keydown", onKeydown, true);
  document.body.appendChild(overlay);
  close.focus();
  return store;
}
