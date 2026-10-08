// The Help menu's About dialog: a 380px card with the product icon, the
// application version - and the build date, when the build defines one -
// each with its own copy button, the license, and the update button.
// "Copy version info" is the primary button: it holds focus on open and
// Enter runs it, putting the version (and date) on the clipboard the way
// Cursor's About dialog does. Every copy confirmation - a row's check
// icon, the primary button's "Copied" - shows only once the clipboard
// write landed. Focus is trapped inside the dialog while it is open,
// Escape and the Close button dismiss it, and focus returns to the element
// that opened it.

import "./about-dialog.css";

import { ICON_CHECK, ICON_COPY } from "@workshop/look/icons";
import { DisposableStore, toDisposable, type IDisposable } from "@workshop/platform/lifecycle";
import type { UpdateService } from "../../services/update-service";
import { copyToClipboard } from "../shared/clipboard";

// The crate version, substituted by the esbuild define in build.mjs and
// the crate's build.rs; a bundle built without the define shows the "dev"
// fallback instead of breaking on a free identifier. The build date has no
// define yet (build.mjs and the build-ui crate must emit identical bundles,
// and a clock-stamped date would break that), so APP_DATE is null and the
// date row stays out of the card until a build defines the date; a row that
// always read "Unknown" would only offer to copy that word.
declare const __APP_VERSION__: string | undefined;
declare const __APP_BUILD_DATE__: string | undefined;
const APP_VERSION = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "dev";
const APP_DATE =
  typeof __APP_BUILD_DATE__ === "string" && __APP_BUILD_DATE__ !== "" ? __APP_BUILD_DATE__ : null;
const LICENSE = "BSL-1.0";

const FOCUSABLE_SELECTOR =
  'button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

/**
 * One labelled value with a copy button; `value` is replaced as the snapshot
 * changes. `onCopy` resolves true once the copy landed; the button's check
 * icon shows only then.
 */
function buildRow(
  field: string,
  label: string,
  copyLabel: string,
  onCopy: () => Promise<boolean>,
): { readonly row: HTMLElement; readonly value: HTMLElement } {
  const row = document.createElement("div");
  row.className = "ws-about-dialog__row";
  row.dataset["field"] = field;
  const name = document.createElement("span");
  name.className = "ws-about-dialog__label";
  name.textContent = label;
  const value = document.createElement("span");
  value.className = "ws-about-dialog__value";
  const copy = document.createElement("button");
  copy.type = "button";
  copy.className = "ws-about-dialog__copy";
  copy.setAttribute("aria-label", copyLabel);
  copy.innerHTML = ICON_COPY;
  copy.addEventListener("click", () => {
    void onCopy().then((copied) => {
      if (copied) {
        copy.innerHTML = ICON_CHECK;
      }
    });
  });
  row.append(name, value, copy);
  return { row, value };
}

/**
 * Opens the About modal; a no-op while one is already open. Returns the
 * disposable that dismisses the dialog - Escape and the Close button
 * dispose it too.
 */
export function showAboutDialog(updates?: UpdateService): IDisposable {
  if (document.querySelector(".ws-about-dialog")) {
    // The open dialog owns its own teardown; there is nothing to release.
    return toDisposable(() => {});
  }
  const invoker = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  const overlay = document.createElement("div");
  overlay.className = "ws-about-dialog-overlay";

  const dialog = document.createElement("section");
  dialog.className = "ws-about-dialog";
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-labelledby", "about-dialog-title");

  const icon = document.createElement("img");
  icon.className = "ws-about-dialog__icon";
  icon.src = "/icons/promptforge-icon.png";
  icon.srcset = "/icons/promptforge-icon.png 1x, /icons/promptforge-icon@2x.png 2x";
  icon.alt = "";

  const title = document.createElement("h2");
  title.id = "about-dialog-title";
  title.className = "ws-about-dialog__title";
  title.textContent = "PromptForge";

  let currentVersion = APP_VERSION;
  const versionRow = buildRow("version", "Version", "Copy version", () =>
    copyToClipboard(currentVersion),
  );
  // The date row exists only when the build defines a date.
  const dateRow =
    APP_DATE === null
      ? null
      : buildRow("date", "Date", "Copy date", () => copyToClipboard(APP_DATE));
  if (dateRow !== null) {
    dateRow.value.textContent = APP_DATE;
  }

  const license = document.createElement("p");
  license.className = "ws-about-dialog__line";
  license.textContent = `License: ${LICENSE}`;

  const check = document.createElement("button");
  check.type = "button";
  check.className = "ws-about-dialog__check button button-secondary";

  const primary = document.createElement("button");
  primary.type = "button";
  primary.className = "ws-about-dialog__primary button button-primary";
  primary.textContent = "Copy version info";

  const close = document.createElement("button");
  close.type = "button";
  close.className = "ws-about-dialog__close button button-secondary";
  close.textContent = "Close";

  const actions = document.createElement("div");
  actions.className = "ws-about-dialog__actions";
  actions.append(check, close, primary);

  // Cursor's update-button labels, one per update phase.
  const renderUpdate = (): void => {
    const snapshot = updates?.snapshot;
    currentVersion = snapshot?.currentVersion || APP_VERSION;
    versionRow.value.textContent = currentVersion;
    if (!snapshot || snapshot.phase === "browser") {
      check.textContent = "Desktop updates unavailable";
      check.disabled = true;
    } else if (snapshot.phase === "unsupported") {
      check.textContent = "Updates are managed by your package manager";
      check.disabled = true;
    } else if (snapshot.phase === "checking") {
      check.textContent = "Checking for Updates...";
      check.disabled = true;
    } else if (snapshot.phase === "available" || snapshot.phase === "dismissed") {
      check.textContent = "Download Update";
      check.disabled = false;
    } else if (snapshot.phase === "error") {
      check.textContent = "Retry update check";
      check.disabled = false;
    } else {
      check.textContent = "Check for Updates...";
      check.disabled = false;
    }
  };

  dialog.append(icon, title, versionRow.row);
  if (dateRow !== null) {
    dialog.append(dateRow.row);
  }
  dialog.append(license, actions);
  overlay.appendChild(dialog);

  const store = new DisposableStore();
  if (updates) {
    store.add(updates.onDidChange(renderUpdate));
  }

  function dismiss(): void {
    store.dispose();
  }

  /**
   * Copies the version (and the date, when there is one) as the dialog's
   * primary action. The button says "Copied" only when the write landed.
   */
  async function copyVersionInfo(): Promise<void> {
    const info =
      APP_DATE === null
        ? `Version: ${currentVersion}`
        : `Version: ${currentVersion}\nDate: ${APP_DATE}`;
    if (await copyToClipboard(info)) {
      primary.textContent = "Copied";
    }
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      dismiss();
      return;
    }
    if (event.key === "Enter") {
      // Enter belongs to whichever other button holds focus (Close, the
      // update button, a copy button); anywhere else it is the primary
      // action, Copy version info.
      const active = document.activeElement;
      const onOtherButton =
        active instanceof HTMLElement &&
        active !== primary &&
        dialog.contains(active) &&
        active.tagName === "BUTTON";
      if (!onOtherButton) {
        event.preventDefault();
        void copyVersionInfo();
      }
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

  // The buttons' listeners are element-owned: they go away with the
  // overlay and need no registration.
  close.addEventListener("click", dismiss);
  primary.addEventListener("click", () => {
    void copyVersionInfo();
  });
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
  primary.focus();
  return store;
}
