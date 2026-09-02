// The Help menu's About dialog: a small themed modal naming the product,
// the application version, and the license. Focus is trapped inside the
// dialog while it is open, Escape and the Close button dismiss it, and
// focus returns to the element that opened it.

import "./about-dialog.css";

import { DisposableStore, toDisposable, type IDisposable } from "../base/lifecycle";
import type { UpdateService } from "../services/update-service";

// Mirrors [workspace.package] version in the workspace Cargo.toml; bump
// together with the crate version.
const APP_VERSION = "0.2.0";
const LICENSE = "BSL-1.0";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/**
 * Opens the About modal; a no-op while one is already open. Returns the
 * disposable that dismisses the dialog - Escape and the Close button
 * dispose it too.
 */
export function showAboutDialog(updates?: UpdateService): IDisposable {
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

  const check = document.createElement("button");
  check.type = "button";
  check.className = "about-dialog__check";
  const renderUpdate = (): void => {
    const snapshot = updates?.snapshot;
    version.textContent = `Version ${snapshot?.currentVersion || APP_VERSION}`;
    if (!snapshot || snapshot.phase === "browser") {
      check.textContent = "Desktop updates unavailable";
      check.disabled = true;
    } else if (snapshot.phase === "unsupported") {
      check.textContent = "Updates are managed by your package manager";
      check.disabled = true;
    } else if (snapshot.phase === "checking") {
      check.textContent = "Checking for updates...";
      check.disabled = true;
    } else if (snapshot.phase === "available" || snapshot.phase === "dismissed") {
      check.textContent = `Show update ${snapshot.version}`;
      check.disabled = false;
    } else if (snapshot.phase === "error") {
      check.textContent = "Retry update check";
      check.disabled = false;
    } else {
      check.textContent = "Check for updates";
      check.disabled = false;
    }
  };

  dialog.append(title, version, license, check, close);
  overlay.appendChild(dialog);

  const store = new DisposableStore();
  if (updates) {
    store.add(updates.onDidChange(renderUpdate));
  }

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
  check.addEventListener("click", () => {
    const snapshot = updates?.snapshot;
    if (snapshot?.phase === "available" || snapshot?.phase === "dismissed") {
      updates?.showAvailable();
      dismiss();
    } else {
      void updates?.checkNow();
    }
  });
  renderUpdate();
  document.addEventListener("keydown", onKeydown, true);
  document.body.appendChild(overlay);
  close.focus();
  return store;
}
