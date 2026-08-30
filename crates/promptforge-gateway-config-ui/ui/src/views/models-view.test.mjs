// Pins the Models view's list side: rows with status dots and badges
// render from the config JSON, the filter chips and debounced search
// narrow the list, the orphan section adopts and deletes unconfigured
// files, and an empty config offers the three entry points.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, settle } from "../harness.mjs";

const ORPHAN = {
  path: "models/stray-7B-Q5_K_S.gguf",
  size_bytes: 4_900_000_000,
  sha256: "a".repeat(64),
};

function fixtureStub(extra = {}) {
  const config = modelsFixture();
  return gatewayStub({
    key: "k",
    config,
    models: ["qwen-common"],
    orphans: [ORPHAN],
    ...extra,
  });
}

test("the model list renders rows, badges, and the orphan section from the config", async () => {
  const stub = fixtureStub();
  const { root } = await bootApp({ key: "k", stub });

  const rows = [...root.querySelectorAll(".model-row")];
  assert.deepEqual(
    rows.map((row) => row.querySelector(".model-name").textContent),
    ["gpt-remote", "llama-leaf", "qwen-common"],
    "every configured model renders, sorted by name",
  );

  const qwen = rows.find((row) => row.textContent.includes("qwen-common"));
  assert.equal(
    qwen.querySelector(".quant-badge").textContent,
    "Q4_K_M",
    "the quant badge is parsed from the GGUF filename",
  );
  assert.equal(qwen.querySelector(".kind-badge").textContent, "chat");
  assert.ok(
    qwen.querySelector(".status-dot").classList.contains("is-ok"),
    "a model the running profile exposes shows the green dot",
  );
  const llama = rows.find((row) => row.textContent.includes("llama-leaf"));
  assert.ok(
    !llama.querySelector(".status-dot").classList.contains("is-ok"),
    "a stopped model keeps the gray dot",
  );

  assert.equal(
    root.querySelector(".orphan-heading")?.textContent,
    "Unconfigured files on disk",
    "the orphan section renders at the list bottom",
  );
  const orphanRow = root.querySelector(".orphan-row");
  assert.match(orphanRow.textContent, /stray-7B-Q5_K_S\.gguf/);
  assert.match(orphanRow.querySelector(".orphan-size").textContent, /4\.6 GiB/);
  assert.ok(orphanRow.querySelector(".orphan-adopt"), "the orphan offers Adopt");
  assert.ok(orphanRow.querySelector(".orphan-delete"), "the orphan offers Delete");
});

test("filter chips and the debounced search narrow the list", async () => {
  const stub = fixtureStub();
  const { root } = await bootApp({ key: "k", stub });

  root.querySelector(".filter-chip[data-filter='local']").click();
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["llama-leaf", "qwen-common"],
    "the Local chip keeps only local models",
  );
  assert.equal(root.querySelector(".orphan-section"), null, "orphans hide under Local");

  root.querySelector(".filter-chip[data-filter='remote']").click();
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["gpt-remote"],
    "the Remote chip keeps only remote models",
  );

  root.querySelector(".filter-chip[data-filter='unconfigured']").click();
  assert.equal(root.querySelectorAll(".model-row").length, 0, "Unconfigured hides the catalog");
  assert.ok(root.querySelector(".orphan-section"), "Unconfigured shows the orphans");

  root.querySelector(".filter-chip[data-filter='all']").click();
  const search = root.querySelector("#models-search");
  search.value = "llama";
  search.dispatchEvent(new search.ownerDocument.defaultView.Event("input"));
  await new Promise((resolve) => setTimeout(resolve, 250));
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["llama-leaf"],
    "the search filters by name after the debounce",
  );
});

test("an empty config renders the empty state with the three entry points", async () => {
  const stub = gatewayStub({ key: "k" });
  const { root } = await bootApp({ key: "k", stub });

  const empty = root.querySelector(".empty-state");
  assert.ok(empty, "the empty state mounts");
  assert.match(empty.textContent, /No models configured/);
  const labels = [...empty.querySelectorAll(".empty-actions .button")].map(
    (button) => button.textContent,
  );
  assert.deepEqual(labels, ["Add Local Model", "Add Remote Model", "Search Hugging Face"]);
});

test("the toolbar Add buttons create a draft and open its detail pane", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".toolbar-add-remote").click();
  await settle();

  assert.equal(dom.window.location.hash, "#/models/new-remote-model");
  assert.ok(root.querySelector(".draft-badge"), "the list marks the draft unsaved");
  assert.equal(root.querySelector(".detail-save").disabled, false, "the draft is saveable");
  assert.ok(
    root.querySelector(".field-row[data-key='upstream']"),
    "the remote draft renders the remote registry",
  );
});

test("adopting an orphan pre-fills an unsaved local model whose save writes the entry", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  root.querySelector(".orphan-adopt").click();
  await settle();

  assert.equal(dom.window.location.hash, "#/models/stray-7B-Q5_K_S");
  const title = root.querySelector(".detail-title");
  assert.equal(title.value, "stray-7B-Q5_K_S", "the name derives from the filename");
  const source = root.querySelector(".field-row[data-key='source'] input");
  assert.equal(source.value, ORPHAN.path, "the source pre-fills with the cache path");
  const sha = root.querySelector(".field-row[data-key='sha256'] input");
  assert.equal(sha.value, ORPHAN.sha256, "the sidecar digest pre-fills the pin");

  const save = root.querySelector(".detail-save");
  assert.equal(save.disabled, false, "a draft is always saveable");
  save.click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.ok(put, "the save PUTs the config shadow");
  const body = JSON.parse(put.init.body);
  const adopted = body.local_model.find((entry) => entry.name === "stray-7B-Q5_K_S");
  assert.equal(adopted?.source, ORPHAN.path, "the adopted entry lands in [[local_model]]");
});

test("deleting an orphan confirms first, then calls the cache delete", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  const doc = dom.window.document;

  root.querySelector(".orphan-delete").click();
  await settle();
  const dialog = doc.querySelector(".confirm-overlay");
  assert.ok(dialog, "a confirm dialog opens");
  assert.match(dialog.textContent, /stray-7B-Q5_K_S\.gguf/, "the dialog names the file");
  assert.match(dialog.textContent, /4\.6 GiB/, "the dialog names the size");

  dialog.querySelector(".button-outline").click();
  await settle();
  assert.equal(
    stub.calls.find((call) => call.init.method === "DELETE"),
    undefined,
    "Cancel deletes nothing",
  );

  root.querySelector(".orphan-delete").click();
  await settle();
  doc.querySelector(".confirm-overlay .button-danger").click();
  await settle();

  const del = stub.calls.find((call) => call.init.method === "DELETE");
  assert.ok(del, "the confirmed delete calls the cache endpoint");
  assert.ok(
    del.url.endsWith(`/v1/cache/${ORPHAN.sha256}`),
    "the delete names the orphan's sha256",
  );
  assert.equal(root.querySelector(".orphan-row"), null, "the refreshed list drops the orphan");
});
