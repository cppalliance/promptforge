// Pins the tab bar's write-path pair: shadows at load raise the
// pending-changes banner and the Apply (N) count, Apply calls the
// config-apply endpoint and clears the pair, and Revert All confirms
// before calling config-revert.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  gatewayStub,
  jsonResponse,
  modelsFixture,
  navigate,
  settle,
} from "../harness.mjs";

/** A stub whose pending view already differs (a previous session's save). */
function dirtyStub() {
  const config = modelsFixture();
  const pending = structuredClone(config);
  pending.local_model[1].description = "saved last session";
  // A wholly-new pending endpoint: its api_key arrives redacted, the way
  // the gateway's pending view serializes every secret.
  pending.endpoint.push({
    id: "new-ep",
    protocol: "openai",
    base_url: "https://api.example.test/v1",
    api_key: "***",
  });
  return gatewayStub({
    key: "k",
    config,
    pending,
    dirty: {
      dirty: true,
      pending_files: ["gateway.toml"],
      changed_sections: ["local_model", "endpoint"],
    },
  });
}

test("shadows at load raise the banner and Apply count, and Apply calls the endpoint", async () => {
  const stub = dirtyStub();
  const { root } = await bootApp({ key: "k", stub });

  const banner = root.querySelector(".banner-pending");
  assert.equal(banner.hidden, false, "the pending-changes banner shows at load");
  assert.match(banner.textContent, /1 pending change from a previous session/);
  assert.ok(banner.querySelector(".banner-apply"), "the banner offers Apply");
  assert.ok(banner.querySelector(".banner-revert"), "the banner offers Revert All");

  const apply = root.querySelector(".apply-button");
  assert.equal(apply.textContent, "Apply (1)", "the count comes from the dirty report");
  assert.ok(root.querySelector(".revert-button"), "Revert All renders beside Apply");

  apply.click();
  await settle();

  const call = stub.calls.find((c) => c.url.endsWith("/admin/config-apply"));
  assert.equal(call?.init.method, "POST", "Apply POSTs config-apply");
  assert.equal(root.querySelector(".apply-button"), null, "a clean report hides the pair");
  assert.equal(root.querySelector(".banner-pending").hidden, true, "the banner clears");
  assert.ok(
    root.ownerDocument.querySelector(".toast-success"),
    "the outcome surfaces as a toast",
  );
});

test("after Apply, a fresh same-session save raises the count but not the banner", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".apply-button").click();
  await settle();
  assert.equal(root.querySelector(".banner-pending").hidden, true, "the banner clears");

  navigate(dom, "#/local/llama-leaf");
  await settle();
  const description = root.querySelector(".field-row[data-key='description'] textarea");
  description.value = "fresh edit";
  description.dispatchEvent(new dom.window.Event("change"));
  await settle();
  root.querySelector(".detail-save").click();
  await settle();

  assert.match(
    root.querySelector(".apply-button")?.textContent ?? "",
    /Apply \(1\)/,
    "the new save raises the Apply count",
  );
  assert.equal(
    root.querySelector(".banner-pending").hidden,
    true,
    "the previous-session banner stays disarmed for same-session saves",
  );
});

test("the banner's Review opens the pending-vs-running diff table", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  const doc = dom.window.document;

  const review = root.querySelector(".banner-review");
  assert.ok(review, "the banner offers Review");
  review.click();
  await settle();

  const table = doc.querySelector(".review-overlay .diff-table");
  assert.ok(table, "Review opens the two-column value table");
  const rows = new Map(
    [...table.querySelectorAll("tbody tr")].map((tr) => [
      tr.querySelector(".diff-path").textContent,
      {
        running: tr.querySelector(".diff-running").textContent,
        pending: tr.querySelector(".diff-pending").textContent,
      },
    ]),
  );
  assert.deepEqual(
    rows.get("local_model[llama-leaf].description"),
    { running: "defined in the leaf", pending: "saved last session" },
    "a changed value renders running against pending",
  );
  assert.equal(
    rows.get("endpoint[new-ep].api_key").pending,
    "***",
    "a pending secret stays redacted in the diff",
  );
  assert.equal(
    rows.get("endpoint[new-ep].api_key").running,
    "(absent)",
    "the running side of a new entry reads absent",
  );

  doc.querySelector(".review-close").click();
  await settle();
  assert.equal(doc.querySelector(".review-overlay"), null, "Close dismisses the dialog");
});

test("the Review dialog notes the redaction blind spot, traps Tab, and restores focus", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  const doc = dom.window.document;

  const review = root.querySelector(".banner-review");
  review.focus();
  review.click();
  await settle();

  const overlay = doc.querySelector(".review-overlay");
  assert.match(
    overlay.querySelector(".review-note").textContent,
    /secret values and staged \.env file edits are not shown/i,
    "the dialog says what the redacted diff cannot show",
  );

  const close = overlay.querySelector(".review-close");
  assert.equal(doc.activeElement, close, "the dialog moves focus to Close on open");
  close.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
  );
  assert.equal(doc.activeElement, close, "Tab wraps inside the aria-modal card");
  close.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", {
      key: "Tab",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }),
  );
  assert.equal(doc.activeElement, close, "Shift+Tab wraps too");

  close.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  assert.equal(doc.querySelector(".review-overlay"), null, "Escape dismisses the dialog");
  assert.equal(doc.activeElement, review, "focus returns to the opener");

  // The backdrop dismisses as well.
  review.click();
  await settle();
  doc.querySelector(".review-overlay").click();
  assert.equal(doc.querySelector(".review-overlay"), null, "the backdrop dismisses the dialog");
});

test("Revert All confirms first, then calls config-revert", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  const doc = dom.window.document;

  root.querySelector(".revert-button").click();
  await settle();
  const dialog = doc.querySelector(".confirm-overlay");
  assert.ok(dialog, "Revert All opens a confirm dialog");
  assert.match(dialog.textContent, /Revert all pending changes\?/);

  dialog.querySelector(".button-outline").click();
  await settle();
  assert.equal(
    stub.calls.find((c) => c.url.endsWith("/admin/config-revert")),
    undefined,
    "Cancel reverts nothing",
  );

  root.querySelector(".revert-button").click();
  await settle();
  doc.querySelector(".confirm-overlay .button-danger").click();
  await settle();

  const call = stub.calls.find((c) => c.url.endsWith("/admin/config-revert"));
  assert.equal(call?.init.method, "POST", "the confirmed revert POSTs config-revert");
  assert.equal(root.querySelector(".apply-button"), null, "the pair clears after revert");
});

/** Confirms the open Revert All dialog and lets the revert settle. */
async function confirmRevert(root, doc) {
  root.querySelector(".revert-button").click();
  await settle();
  doc.querySelector(".confirm-overlay .button-danger").click();
  await settle();
}

test("Revert All discards a model's unsaved edits along with the shadows", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/local/llama-leaf");
  await settle();
  const description = root.querySelector(".field-row[data-key='description'] textarea");
  description.value = "typed but never saved";
  description.dispatchEvent(new dom.window.Event("change"));
  await settle();
  assert.ok(root.querySelector(".dirty-dot"), "the unsaved edit raises the dirty dot");
  assert.equal(root.querySelector(".detail-save").disabled, false, "Save arms for the edit");

  await confirmRevert(root, dom.window.document);

  assert.equal(root.querySelector(".apply-button"), null, "the pair clears after revert");
  assert.equal(root.querySelector(".dirty-dot"), null, "the dirty dot goes with the revert");
  assert.equal(root.querySelector(".field-reset"), null, "the field reset goes too");
  assert.equal(root.querySelector(".detail-save").disabled, true, "Save disarms");
  assert.equal(
    root.querySelector(".field-row[data-key='description'] textarea").value,
    "defined in the leaf",
    "the field shows the running value again",
  );
});

test("a failed Revert All keeps the unsaved edits", async () => {
  const stub = dirtyStub();
  const fetch = stub.fetchFn;
  stub.fetchFn = async (input, init = {}) => {
    if (String(input).endsWith("/admin/config-revert")) {
      return jsonResponse({ error: "the shadow is locked" }, 500);
    }
    return fetch(input, init);
  };
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/local/llama-leaf");
  await settle();
  const description = root.querySelector(".field-row[data-key='description'] textarea");
  description.value = "typed but never saved";
  description.dispatchEvent(new dom.window.Event("change"));
  await settle();

  await confirmRevert(root, dom.window.document);
  assert.ok(root.ownerDocument.querySelector(".toast-error"), "the failure surfaces as a toast");
  // A failed revert notifies nobody, so the pane still shows its last
  // paint; a route round-trip repaints from the store's real state.
  navigate(dom, "#/local");
  await settle();
  navigate(dom, "#/local/llama-leaf");
  await settle();

  assert.ok(root.querySelector(".dirty-dot"), "the edit survives the failed revert");
  assert.equal(root.querySelector(".detail-save").disabled, false, "Save stays armed");
  assert.equal(
    root.querySelector(".field-row[data-key='description'] textarea").value,
    "typed but never saved",
    "the field keeps the typed value",
  );
  assert.ok(root.querySelector(".apply-button"), "the pair stays while the shadows remain");
});

test("Revert All discards a draft model", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/local");
  await settle();
  root.querySelector(".toolbar-add-local").click();
  await settle();
  const names = () => [...root.querySelectorAll(".model-name")].map((el) => el.textContent);
  assert.ok(names().includes("new-local-model"), "the draft joins the list before the revert");

  await confirmRevert(root, dom.window.document);

  assert.ok(!names().includes("new-local-model"), "the draft goes with the revert");
});

test("Revert All discards the Settings view's unsaved edits", async () => {
  const stub = dirtyStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  const bind = root.querySelector(".field-row[data-key='bind'] input");
  const original = bind.value;
  bind.value = "0.0.0.0:9999";
  bind.dispatchEvent(new dom.window.Event("change"));
  await settle();
  assert.ok(root.querySelector(".dirty-dot"), "the unsaved edit raises the dirty dot");
  assert.equal(root.querySelector(".card-save").disabled, false, "Save arms for the edit");

  await confirmRevert(root, dom.window.document);

  assert.equal(root.querySelector(".dirty-dot"), null, "the dirty dot goes with the revert");
  assert.equal(root.querySelector(".card-save").disabled, true, "Save disarms");
  assert.equal(
    root.querySelector(".field-row[data-key='bind'] input").value,
    original,
    "the field shows the running value again",
  );
});
