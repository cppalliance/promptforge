// Pins the apply overlay against the gateway's real progress wire: the
// `GET /admin/progress` stream carries raw hub `ProgressEvent` JSON
// (`state` is serde's externally tagged `EventState`), so a `Begun`
// leaf labelled with a switch stage lights that stage and nothing else
// does; a `<stage>/<model>/download` leaf drives a detail row under the
// active stage (name, integer percent, verify and start labels) until
// the stage ends; the card's Cancel posts the active-command cancel
// once, stays disabled (also once the apply settles), a refused cancel
// toasts on its own without closing the card, and the apply route's
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

/** A hub `ProgressEvent` frame as `event_line` in the gateway serializes it. */
function hubEvent(label, state) {
  return { operation: 7, path: label, label, state };
}

/** A hub frame for a leaf path whose label differs from the path. */
function leafEvent(path, state) {
  return { operation: 7, path, label: path.split("/").at(-1), state };
}

/**
 * Boots a dirty shell whose progress stream is pushable and whose
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

/** The `data-stage` ids of every row carrying the `is-active` class. */
function activeStages(overlay) {
  return [...overlay.querySelectorAll(".stage.is-active")].map((row) => row.dataset.stage);
}

test("a hub Begun event labelled with a switch stage lights that stage; other frames change nothing", async () => {
  const { overlay, progress, settleApply } = await bootApplying();
  assert.deepEqual(activeStages(overlay), [], "no stage is active before the gateway reports one");
  assert.deepEqual(
    [...overlay.querySelectorAll(".stage")].map((row) => row.dataset.stage),
    ["loading-profile", "downloading-models", "stopping-models", "starting-models"],
    "the card lists the four switch stages the gateway can register",
  );

  // The download stage the switch registers while the new profile's
  // weights stage into the cache lights like the others.
  progress.push(hubEvent("downloading-models", { Begun: { weight: 5.0 } }));
  await settle();
  const downloading = overlay.querySelector('.stage[data-stage="downloading-models"]');
  assert.deepEqual(activeStages(overlay), ["downloading-models"], "the download stage is active");
  assert.ok(downloading.querySelector(".spinner"), "the active download stage shows the spinner");

  progress.push(hubEvent("stopping-models", { Begun: { weight: 2.0 } }));
  await settle();
  const stopping = overlay.querySelector('.stage[data-stage="stopping-models"]');
  assert.deepEqual(activeStages(overlay), ["stopping-models"], "the Begun stage is active");
  assert.ok(stopping.querySelector(".spinner"), "the active stage shows the spinner");
  assert.ok(downloading.classList.contains("is-done"), "the earlier stage is marked done");

  // A non-Begun frame for a stage and a Begun frame for a non-stage leaf
  // (a model download) leave the stage list exactly as it was.
  progress.push(hubEvent("starting-models", { Updated: { fraction: 0.5 } }));
  progress.push(hubEvent("downloading-models/qwen/download", { Begun: { weight: 1.0 } }));
  progress.push(hubEvent("starting-models", { Finished: { ok: true } }));
  progress.push({ stage: "starting-models" });
  await settle();
  assert.deepEqual(activeStages(overlay), ["stopping-models"], "only the Begun stage is active");
  assert.equal(overlay.querySelectorAll(".stage").length, 4, "no row was appended");
  assert.ok(stopping.querySelector(".spinner"), "the spinner is still on the Begun stage");

  settleApply(jsonResponse({ applied: ["gateway.toml"], reloaded: true, restart_required: false }));
  await settle();
  progress.end();
});

test("a download leaf drives the detail row through percent, verify, and start; the stage end clears it", async () => {
  const { overlay, progress, settleApply } = await bootApplying();
  progress.push(hubEvent("downloading-models", { Begun: { weight: 5.0 } }));
  await settle();
  assert.equal(overlay.querySelector(".stage-detail"), null, "no detail row before a download leaf");

  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Begun: { weight: 1.0 } }));
  await settle();
  const detail = overlay.querySelector(".stage-detail");
  assert.ok(detail, "the download Begun shows the detail row");
  assert.equal(
    detail.querySelector(".stage-detail-label").textContent,
    "Downloading glm-4-9b",
    "the row names the model",
  );
  const bar = detail.querySelector(".progress");
  assert.ok(bar, "the row carries the shared inline progress bar");
  const activeRow = overlay.querySelector('.stage[data-stage="downloading-models"]');
  assert.equal(activeRow.nextElementSibling, detail, "the row sits under the active stage");

  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Updated: { fraction: 0.42 } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-percent").textContent, "42%");
  assert.equal(bar.getAttribute("aria-valuenow"), "42", "the bar reports the integer percent");

  // A fraction outside 0..1 clamps instead of overflowing the bar.
  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Updated: { fraction: 1.7 } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-percent").textContent, "100%");
  assert.equal(bar.getAttribute("aria-valuenow"), "100");

  // A frame flood updates the existing nodes; nothing is re-created.
  assert.equal(detail.querySelector(".progress"), bar, "the bar node is updated in place");
  assert.equal(overlay.querySelector(".stage-detail"), detail, "the row node is updated in place");

  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Finished: { ok: true } }));
  await settle();
  assert.equal(
    detail.querySelector(".stage-detail-label").textContent,
    "Verifying glm-4-9b",
    "the finished download flips the row to verifying",
  );

  progress.push(hubEvent("starting-models", { Begun: { weight: 5.0 } }));
  await settle();
  assert.equal(overlay.querySelector(".stage-detail"), null, "the stage change clears the row");

  progress.push(leafEvent("starting-models/glm-4-9b/ready", { Begun: { weight: 2.0 } }));
  await settle();
  assert.equal(
    overlay.querySelector(".stage-detail-label")?.textContent,
    "Starting glm-4-9b",
    "the ready leaf flips the row to starting",
  );

  settleApply(jsonResponse({ applied: ["gateway.toml"], reloaded: true, restart_required: false }));
  await settle();
  progress.end();
});

test("only the most recent download leaf drives the row; unrelated paths change nothing", async () => {
  const { overlay, progress, settleApply } = await bootApplying();
  progress.push(hubEvent("downloading-models", { Begun: { weight: 5.0 } }));
  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Begun: { weight: 1.0 } }));
  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Updated: { fraction: 0.42 } }));
  await settle();
  const detail = overlay.querySelector(".stage-detail");
  assert.equal(detail.querySelector(".stage-detail-percent").textContent, "42%");

  // Frames for other leaves of the same model and for paths outside the
  // known stages leave the row exactly as it was.
  progress.push(leafEvent("downloading-models/glm-4-9b/verify", { Updated: { fraction: 0.9 } }));
  progress.push(leafEvent("local-models/qwen/download", { Begun: { weight: 1.0 } }));
  progress.push(leafEvent("download", { Updated: { fraction: 0.9 } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-label").textContent, "Downloading glm-4-9b");
  assert.equal(detail.querySelector(".stage-detail-percent").textContent, "42%");

  // A second model's download Begun wins the row and resets the bar.
  progress.push(leafEvent("downloading-models/qwen/download", { Begun: { weight: 1.0 } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-label").textContent, "Downloading qwen");
  assert.equal(detail.querySelector(".stage-detail-percent").textContent, "0%");

  // The earlier leaf's frames no longer drive the bar.
  progress.push(leafEvent("downloading-models/glm-4-9b/download", { Updated: { fraction: 0.9 } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-percent").textContent, "0%");

  // A model name containing a slash (the config validates only that it
  // is non-empty) displays in full: the stage is the first segment, the
  // leaf the last, the model everything between.
  progress.push(leafEvent("downloading-models/org/model/download", { Begun: { weight: 1.0 } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-label").textContent, "Downloading org/model");
  progress.push(leafEvent("downloading-models/org/model/download", { Finished: { ok: true } }));
  await settle();
  assert.equal(detail.querySelector(".stage-detail-label").textContent, "Verifying org/model");

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
    "the overlay's failure note carries the same wording",
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
