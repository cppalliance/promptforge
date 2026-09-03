// The pending-changes Review dialog [INVENTED, flagged in the plan]: a
// dimmed overlay centering a card with a two-column value table - one
// row per changed path, running value against pending value. Both
// config views arrive with secrets redacted to "***", so the table can
// never show credential material - a note says so, because a changed
// secret or a staged .env shadow leaves no visible row. Escape, the
// backdrop, and the Close button dismiss it; Tab stays inside the card;
// focus returns to the opener.

import type { DiffRow } from "../services/config-store";

/** Renders one diff value: strings verbatim, everything else as JSON. */
function renderValue(value: unknown): string {
  if (value === undefined) {
    return "(absent)";
  }
  return typeof value === "string" ? value : JSON.stringify(value);
}

/** Opens the Review dialog over `host` with the diff `rows`. */
export function openReviewDiff(host: HTMLElement, rows: DiffRow[]): void {
  const overlay = document.createElement("div");
  overlay.className = "overlay review-overlay";

  const card = document.createElement("section");
  card.className = "modal review-modal";
  card.setAttribute("role", "dialog");
  card.setAttribute("aria-modal", "true");

  const heading = document.createElement("h2");
  heading.id = "review-title";
  heading.textContent = "Pending changes";
  card.setAttribute("aria-labelledby", heading.id);
  card.append(heading);

  if (rows.length === 0) {
    // Secret edits and staged .env shadows raise the pending count but
    // leave no visible row (secrets arrive redacted on both sides), so
    // an empty table must not claim the views match.
    const empty = document.createElement("p");
    empty.className = "view-empty";
    empty.textContent = "No visible value changes.";
    card.append(empty);
  } else {
    const table = document.createElement("table");
    table.className = "diff-table";
    const caption = document.createElement("caption");
    caption.className = "visually-hidden";
    caption.textContent = "Pending configuration changes: running value against pending value";
    const head = document.createElement("thead");
    const headRow = document.createElement("tr");
    for (const label of ["Path", "Running", "Pending"]) {
      const th = document.createElement("th");
      th.scope = "col";
      th.textContent = label;
      headRow.append(th);
    }
    head.append(headRow);
    const body = document.createElement("tbody");
    for (const row of rows) {
      const tr = document.createElement("tr");
      const path = document.createElement("th");
      path.scope = "row";
      path.className = "diff-path";
      path.textContent = row.path;
      const running = document.createElement("td");
      running.className = "diff-running";
      running.textContent = renderValue(row.running);
      const pending = document.createElement("td");
      pending.className = "diff-pending";
      pending.textContent = renderValue(row.pending);
      tr.append(path, running, pending);
      body.append(tr);
    }
    table.append(caption, head, body);
    card.append(table);
  }

  const note = document.createElement("p");
  note.className = "field-help review-note";
  note.textContent =
    "Changed secret values and staged .env file edits are not shown here: secrets stay redacted.";
  card.append(note);

  const actions = document.createElement("div");
  actions.className = "modal-actions";
  const close = document.createElement("button");
  close.type = "button";
  close.className = "button button-outline review-close";
  close.textContent = "Close";
  actions.append(close);
  card.append(actions);
  overlay.append(card);
  host.append(overlay);

  // Duck-typed: the HTMLElement global is absent under node --test.
  const opener = document.activeElement as HTMLElement | null;
  const restore = opener && typeof opener.focus === "function" ? opener : null;

  const dismiss = (): void => {
    overlay.remove();
    if (restore?.isConnected) {
      restore.focus();
    }
  };
  close.addEventListener("click", dismiss);
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) {
      dismiss();
    }
  });
  // The overlay hears Escape wherever focus sits, and the Tab trap keeps
  // focus inside the aria-modal card (the convention the profiles-view
  // dialog and confirm-modal established).
  overlay.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      dismiss();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const controls = [...card.querySelectorAll<HTMLElement>("button, [href], input, select")];
    const first = controls[0];
    const last = controls[controls.length - 1];
    if (!first || !last) {
      return;
    }
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });
  close.focus();
}
