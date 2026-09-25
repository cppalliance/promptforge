// Pins the apply overlay against the gateway's real progress wire: the
// `GET /admin/progress` stream sends `Progress` snapshots
// (`{"busy": bool, "text": string}`), so a busy snapshot's text becomes
// the card's activity row, updated in place, and an idle one shows the
// waiting text; the card's Cancel posts the active-command cancel once,
// stays disabled (also once the apply settles), a refused cancel toasts
// on its own without closing the card, and the apply route's
// `apply_cancelled` refusal words the toast as a cancellation rather
// than a failure.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  gatewayStub,
  jsonResponse,
  modelsFixture,
  settle,
  sseChannel,
} from "../harness.mjs";

const CANCELLED_TOAST = "Apply cancelled - your pending changes are still staged";

/** A `Progress` snapshot frame as the gateway's `snapshot_line` serializes it. */
function snapshot(busy, text) {
  return { busy, text };
}

/**
 * Boots a dirty desk whose progress stream is pushable and whose
 * config-apply reply waits until the test settles it, so the overlay
 * stays open while events and clicks arrive. `cancelReply`, when given,
 * answers `POST /admin/queue/cancel` instead of the stub.
 */
async function bootApplying({ cancelReply } = {}) {
  const stub = gatewayStub({
    key: "k",
    config: modelsFixture(),
    pending: modelsFixture(),
    dirty: { dirty: true, pending_files: ["gateway.toml"], changed_sections: [] },
  });
  const progress = sseChannel();
  let settleApply;
  const applyReply = new Promise((resolve) => {
    settleApply = resolve;
  });
  const fetchFn = (url, init) => {
    if (String(url).endsWith("/admin/progress")) {
      return Promise.resolve(progress.response);
    }
    if (String(url).endsWith("/admin/config-apply")) {
      return applyReply;
    }
    if (cancelReply && String(url).endsWith("/admin/queue/cancel")) {
      return Promise.resolve(cancelReply());
    }
    return stub.fetchFn(url, init);
  };
  const { root } = await bootApp({ key: "k", stub: { fetchFn } });
  root.querySelector(".apply-button").click();
  await settle();
  const overlay = root.querySelector(".apply-overlay");
  assert.ok(overlay, "Apply opens the overlay");
  return { root, stub, progress, overlay, settleApply };
}

test("a busy snapshot's text drives the activity row in place; an idle one shows the waiting text", async () => {
  const { overlay, progress, settleApply } = await bootApplying();
  const rows = [...overlay.querySelectorAll(".stage")];
  assert.equal(rows.length, 1, "the card shows one activity row");
  const row = rows[0];
  const label = row.querySelector(".stage-label");
  assert.ok(row.classList.contains("is-active"), "the row is active from the opening");
  assert.ok(row.querySelector(".spinner"), "the active row shows the spinner");
  assert.equal(label.textContent, "Waiting for the gateway", "nothing reported yet");

  // The stream opens with the current snapshot: idle while the apply
  // command waits to start.
  progress.push(snapshot(false, ""));
  await settle();
  assert.equal(label.textContent, "Waiting for the gateway", "an idle snapshot keeps waiting");

  progress.push(snapshot(true, "load-profile: main"));
  await settle();
  assert.equal(label.textContent, "load-profile: main", "a busy snapshot's text is the row");

  progress.push(snapshot(true, "Downloading glm-4-9b 42%"));
  progress.push(snapshot(true, "Downloading glm-4-9b 43%"));
  progress.push(snapshot(true, "Applying configuration"));
  await settle();
  assert.equal(label.textContent, "Applying configuration", "the newest text wins");
  assert.equal(overlay.querySelector(".stage-label"), label, "the label node is updated in place");
  assert.equal(overlay.querySelectorAll(".stage").length, 1, "no row was appended");
  assert.ok(row.querySelector(".spinner"), "the spinner stays while the apply runs");

  // A frame that is not a snapshot reads as idle and never throws.
  progress.push({ stage: "starting-models" });
  await settle();
  assert.equal(label.textContent, "Waiting for the gateway", "a malformed frame reads as idle");

  settleApply(jsonResponse({ applied: ["gateway.toml"], reloaded: true, restart_required: false }));
  await settle();
  progress.end();
});

test("a busy snapshot with empty text shows the waiting text rather than a blank row", async () => {
  const { overlay, progress, settleApply } = await bootApplying();
  progress.push(snapshot(true, "Downloading qwen 10%"));
  await settle();
  const label = overlay.querySelector(".stage-label");
  assert.equal(label.textContent, "Downloading qwen 10%");
  progress.push(snapshot(true, ""));
  await settle();
  assert.equal(label.textContent, "Waiting for the gateway", "an empty text never blanks the row");
  settleApply(jsonResponse({ applied: ["gateway.toml"], reloaded: true, restart_required: false }));
  await settle();
  progress.end();
});

test("Cancel posts the active-command cancel once, stays disabled, and the cancelled refusal words the toast", async () => {
  const { root, stub, overlay, progress, settleApply } = await bootApplying();
  const cancel = overlay.querySelector(".apply-overlay-cancel");
  assert.ok(cancel, "the card offers Cancel");
  assert.equal(cancel.disabled, false, "Cancel starts enabled");

  cancel.click();
  await settle();
  assert.equal(stub.state.cancelActiveCalls, 1, "Cancel POSTs /admin/queue/cancel");
  assert.equal(cancel.disabled, true, "Cancel disables after the click");
  cancel.click();
  await settle();
  assert.equal(stub.state.cancelActiveCalls, 1, "a second click posts nothing more");
  assert.equal(cancel.disabled, true, "Cancel stays disabled");
  assert.ok(root.contains(overlay), "the overlay waits for the apply route to settle");

  settleApply(
    jsonResponse(
      {
        error: {
          message: "apply cancelled; the pending changes are still staged, retry Apply",
          type: "server_error",
          code: "apply_cancelled",
        },
      },
      503,
    ),
  );
  await settle();
  const toast = root.ownerDocument.querySelector(".toast-error");
  assert.ok(toast, "the cancelled apply raises an error toast");
  assert.equal(toast.textContent, CANCELLED_TOAST, "the toast names the cancellation");
  assert.equal(
    overlay.querySelector(".field-error")?.textContent,
    CANCELLED_TOAST,
    "the overlay's failure note uses the same wording",
  );
  progress.end();
});

test("a non-cancelled apply refusal keeps the gateway's own message and retires Cancel", async () => {
  const { root, overlay, progress, settleApply } = await bootApplying();
  const cancel = overlay.querySelector(".apply-overlay-cancel");
  assert.equal(cancel.disabled, false, "Cancel is live while the apply runs");
  settleApply(
    jsonResponse(
      { error: { message: "the reload failed: boom", type: "server_error", code: "apply_reload_failed" } },
      500,
    ),
  );
  await settle();
  const toast = root.ownerDocument.querySelector(".toast-error");
  assert.equal(toast?.textContent, "the reload failed: boom", "the failure toast is unchanged");
  assert.equal(cancel.disabled, true, "an unclicked Cancel disables once the apply has failed");
  progress.end();
});

test("a refused cancel request raises its own toast and leaves the overlay running", async () => {
  const { root, stub, overlay, progress, settleApply } = await bootApplying({
    cancelReply: () =>
      jsonResponse({ error: { message: "nothing to cancel", type: "invalid_request_error" } }, 409),
  });
  overlay.querySelector(".apply-overlay-cancel").click();
  await settle();
  assert.equal(stub.state.cancelActiveCalls, 0, "the refusal came from the test, not the stub");
  const toast = root.ownerDocument.querySelector(".toast-error");
  assert.equal(toast?.textContent, "nothing to cancel", "the cancel refusal is reported");
  assert.ok(root.contains(overlay), "the overlay stays up: the apply itself is still in flight");
  assert.equal(overlay.querySelector(".field-error"), null, "no failure note: the apply did not fail");

  settleApply(jsonResponse({ applied: ["gateway.toml"], reloaded: true, restart_required: false }));
  await settle();
  assert.equal(root.contains(overlay), false, "the apply's success still closes the overlay");
  progress.end();
});
