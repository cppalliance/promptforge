// Pins the tab bar's write-path pair: shadows at load raise the
// pending-changes banner and the Apply (N) count, Apply calls the
// config-apply endpoint and clears the pair, and Revert All confirms
// before calling config-revert.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

/** A stub whose pending view already differs (a previous session's save). */
function dirtyStub() {
  const config = modelsFixture();
  const pending = structuredClone(config);
  pending.local_model[1].description = "saved last session";
  return gatewayStub({
    key: "k",
    config,
    pending,
    dirty: {
      dirty: true,
      pending_files: ["profiles/default.toml"],
      changed_sections: ["local_model"],
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

  navigate(dom, "#/models/llama-leaf");
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
