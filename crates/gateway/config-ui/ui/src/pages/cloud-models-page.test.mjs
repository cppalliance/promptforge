// Pins the Cloud tab: canonical rows with snapshot disclosures, the
// disabled add action on endpoint-less providers, the confirm-details
// dialog (pre-fill, the required-context path, verbatim shadow-save
// errors), and the in-place re-render when the sheet lands.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  cloudSheetFixture,
  gatewayStub,
  jsonResponse,
  modelsFixture,
  navigate,
  settle,
} from "../harness.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Boots the desk with the sheet stubbed and lands on the Cloud tab. */
async function openCloud(stubOptions = {}) {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), ...stubOptions });
  const { dom, root } = await bootApp({ key: "k", stub, options: { sheetPollMs: 10 } });
  navigate(dom, "#/cloud");
  await settle();
  return { dom, root, stub };
}

/** Selects a provider in the view's dropdown: opens the trigger, clicks the row. */
function chooseProvider(dom, root, name) {
  root.querySelector(".cloud-provider-select .select").click();
  root.querySelector(`.cloud-provider-select .menu-item[data-value='${name}']`).click();
}

test("canonical rows render with the id beneath the name and a snapshot disclosure", async () => {
  const { root } = await openCloud({ cloudModels: cloudSheetFixture() });
  const rows = [...root.querySelectorAll(".cloud-table tbody tr.cloud-row")];
  assert.deepEqual(
    rows.map((row) => row.querySelector(".cloud-name-primary").textContent),
    ["Claude Fable 5.1", "Claude Opus 5"],
    "the default provider's canonical rows, variants hidden",
  );
  assert.equal(
    rows[0].querySelector(".cloud-name-id").textContent,
    "claude-fable-5-1",
    "the id renders beneath a differing display name",
  );
  const toggle = rows[0].querySelector(".cloud-variants-toggle");
  assert.equal(toggle.textContent, "+2 snapshots");
  assert.equal(root.querySelectorAll(".cloud-variant-row").length, 0);
  toggle.click();
  const variants = [...root.querySelectorAll(".cloud-variant-row")];
  assert.equal(variants.length, 2, "the disclosure expands the snapshot rows");
  assert.match(variants[0].textContent, /2026-09-01/);
});

test("the provider and family dropdowns are the app's own listboxes, tier-grouped", async () => {
  const { dom, root } = await openCloud({ cloudModels: cloudSheetFixture() });
  const provider = root.querySelector(".cloud-provider-select");
  assert.equal(provider.querySelector("select"), null, "no native select remains");
  assert.equal(
    provider.querySelector(".menu.dropdown-menu[role='listbox']").getAttribute("aria-labelledby"),
    "cloud-provider",
  );
  const labels = [...provider.querySelectorAll(".menu-group-label")].map((el) => el.textContent);
  assert.ok(labels.length >= 2, "at least two tiers render as group headers");
  assert.deepEqual(labels, [...new Set(labels)], "each tier header renders once");
  assert.ok(labels.includes("Prime"), "tier names capitalize");
  const family = root.querySelector(".cloud-family-select");
  assert.equal(family.querySelector(".select").id, "cloud-family");
  const familyRows = [...family.querySelectorAll(".menu-item")];
  assert.equal(familyRows[0].dataset.value, "");
  assert.equal(familyRows[0].textContent, "All families");
  assert.ok(familyRows.length > 1, "the selected provider's families follow");
  family.querySelector(".select").click();
  familyRows[1].click();
  await settle();
  assert.equal(
    root.querySelector(".cloud-family-select .select").value,
    familyRows[1].dataset.value,
    "choosing a family re-renders with it selected",
  );
  chooseProvider(dom, root, "acme");
  await settle();
  assert.equal(root.querySelector(".cloud-provider-select .select").value, "acme");
  assert.equal(
    root.querySelector(".cloud-family-select .select").value,
    "",
    "a provider change resets the family filter",
  );
});

test("providers without an OpenAI-compatible endpoint render the disabled add and reason", async () => {
  const { dom, root } = await openCloud({ cloudModels: cloudSheetFixture() });
  const stt = [...root.querySelectorAll(".cloud-kind")].find((chip) => chip.dataset.kind === "transcription");
  stt.click();
  await settle();
  const trigger = root.querySelector(".cloud-provider-select .select");
  assert.equal(trigger.id, "cloud-provider", "the hidden label points at the trigger");
  assert.equal(trigger.value, "deepgram", "the only STT provider selects itself");
  assert.equal(
    root.querySelector(".cloud-provider-select .menu-item[aria-selected='true']").dataset.value,
    "deepgram",
  );
  const row = root.querySelector(".cloud-table tbody tr.cloud-row");
  assert.match(row.querySelector(".cloud-name-primary").textContent, /Nova 3/);
  const add = row.querySelector(".cloud-add");
  assert.equal(add.disabled, true);
  assert.match(row.textContent, /no OpenAI-compatible endpoint/);
});

test("the dialog pre-fills from the sheet and stages the merged document", async () => {
  const { dom, root, stub } = await openCloud({ cloudModels: cloudSheetFixture() });
  root.querySelector(".cloud-row .cloud-add").click();
  await settle();
  const dialog = dom.window.document.querySelector(".cloud-add-overlay");
  assert.ok(dialog, "the confirm-details dialog opens");
  assert.equal(dialog.querySelector(".cloud-add-name").value, "Claude Fable 5.1");
  assert.equal(dialog.querySelector(".cloud-add-context").value, "200000");
  dialog.querySelector(".cloud-add-submit").click();
  await settle();
  const puts = stub.calls.filter(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.equal(puts.length, 1, "the merged document stages via PUT /admin/config");
  const body = JSON.parse(puts[0].init.body);
  assert.ok(
    body.endpoint.some(
      (entry) => entry.id === "anthropic" && entry.api_key === "${ANTHROPIC_API_KEY}",
    ),
  );
  const added = body.model.find((entry) => entry.upstream === "claude-fable-5-1");
  assert.ok(added, "the model entry is in the payload");
  assert.equal(added.context, 200000);
  assert.equal(
    dom.window.document.querySelector(".cloud-add-overlay"),
    null,
    "a staged merge closes the dialog",
  );
});

test("the dialog requires operator context when the sheet lacks one", async () => {
  const { dom, root, stub } = await openCloud({ cloudModels: cloudSheetFixture() });
  chooseProvider(dom, root, "acme");
  await settle();
  root.querySelector(".cloud-row .cloud-add").click();
  await settle();
  const dialog = dom.window.document.querySelector(".cloud-add-overlay");
  assert.equal(dialog.querySelector(".cloud-add-context").value, "", "nothing to pre-fill");
  dialog.querySelector(".cloud-add-submit").click();
  await settle();
  assert.match(dialog.querySelector(".cloud-add-error").textContent, /context/);
  assert.ok(dialog.isConnected, "the dialog stays open on the rejection");
  assert.equal(
    stub.calls.filter((call) => call.url.endsWith("/admin/config") && call.init.method === "PUT")
      .length,
    0,
    "no PUT left the dialog",
  );
  const context = dialog.querySelector(".cloud-add-context");
  context.value = "8192";
  context.dispatchEvent(new dom.window.Event("input"));
  dialog.querySelector(".cloud-add-submit").click();
  await settle();
  const puts = stub.calls.filter(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.equal(puts.length, 1);
  assert.equal(
    JSON.parse(puts[0].init.body).model.find((entry) => entry.upstream === "acme-1").context,
    8192,
  );
});

test("a shadow-save refusal surfaces verbatim in the dialog", async () => {
  const { dom, root } = await openCloud({
    cloudModels: cloudSheetFixture(),
    onPutConfig: () =>
      jsonResponse(
        { error: { message: "unknown field `typo`", code: "config_invalid" } },
        422,
      ),
  });
  root.querySelector(".cloud-row .cloud-add").click();
  await settle();
  const dialog = dom.window.document.querySelector(".cloud-add-overlay");
  dialog.querySelector(".cloud-add-submit").click();
  await settle();
  assert.equal(dialog.querySelector(".cloud-add-error").textContent, "unknown field `typo`");
  assert.ok(dialog.isConnected, "the dialog stays open for correction");
});

test("every close path removes the document Escape listener", async () => {
  const { dom, root } = await openCloud({ cloudModels: cloudSheetFixture() });
  const doc = dom.window.document;
  const removed = [];
  const originalRemove = doc.removeEventListener.bind(doc);
  doc.removeEventListener = (type, listener, options) => {
    removed.push(type);
    return originalRemove(type, listener, options);
  };
  root.querySelector(".cloud-row .cloud-add").click();
  await settle();
  doc.querySelector(".cloud-add-overlay .button-outline").click();
  assert.equal(doc.querySelector(".cloud-add-overlay"), null, "Cancel closes the dialog");
  assert.ok(removed.includes("keydown"), "Cancel removes the Escape listener");
  root.querySelector(".cloud-row .cloud-add").click();
  await settle();
  doc.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key: "Escape" }));
  assert.equal(doc.querySelector(".cloud-add-overlay"), null, "Escape still closes the dialog");
});

test("a view open while the sheet downloads re-renders in place when it lands", async () => {
  let serve = false;
  const stub = gatewayStub({
    key: "k",
    config: modelsFixture(),
    onCloudModels: () => {
      if (!serve) {
        return jsonResponse(
          { error: { message: "the sheet is downloading", code: "cloud_models_loading" } },
          503,
        );
      }
      return jsonResponse(cloudSheetFixture());
    },
  });
  const { dom, root } = await bootApp({ key: "k", stub, options: { sheetPollMs: 10 } });
  navigate(dom, "#/cloud");
  await settle();
  assert.match(root.querySelector("main").textContent, /[Ll]oading/);
  assert.equal(root.querySelector(".cloud-table"), null);
  serve = true;
  await sleep(150);
  await settle();
  assert.ok(root.querySelector(".cloud-table"), "the table renders once the sheet lands");
  assert.match(root.querySelector("main").textContent, /2026-09-14T13:00:00Z/, "generated_at shows");
});
