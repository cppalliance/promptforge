import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

const ORPHAN = {
  path: "models/stray-7B-Q5_K_S.gguf",
  size_bytes: 4_900_000_000,
  sha256: "a".repeat(64),
};

async function open(config = modelsFixture(), extra = {}) {
  const stub = gatewayStub({
    key: "k",
    config,
    models: ["gpt-remote", "qwen-common", "whisper-base-en"],
    orphans: [ORPHAN],
    ...extra,
  });
  const booted = await bootApp({ key: "k", stub });
  await settle();
  return { ...booted, stub };
}

test("Local and Remote tabs render only their catalog subsets", async () => {
  const { dom, root } = await open();
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["llama-leaf", "qwen-common", "whisper-base-en"],
  );
  assert.ok(root.querySelector(".orphan-section"), "Local absorbs unconfigured files");

  navigate(dom, "#/remote");
  await settle();
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["gpt-remote"],
  );
  assert.equal(root.querySelector(".orphan-section"), null, "Remote never shows local files");
});

test("STT entries carry the Mic badge and implicit non-editable kind", async () => {
  const { dom, root } = await open();
  const row = [...root.querySelectorAll(".model-row")].find((entry) =>
    entry.textContent.includes("whisper-base-en"),
  );
  assert.equal(row.querySelector(".source-icon").dataset.icon, "mic");
  assert.equal(row.querySelector(".kind-badge").textContent, "stt");

  navigate(dom, "#/local/whisper-base-en");
  await settle();
  assert.equal(root.querySelector(".field-row[data-key='kind']"), null);
  assert.equal(root.querySelector(".field-row[data-key='role'] .select").value, "interim");
});

test("Local secondary filters narrow Chat and STT without hiding unconfigured files", async () => {
  const { root } = await open();
  root.querySelector(".filter-chip[data-filter='chat']").click();
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["llama-leaf", "qwen-common"],
  );
  assert.ok(root.querySelector(".orphan-section"));

  root.querySelector(".filter-chip[data-filter='stt']").click();
  assert.deepEqual(
    [...root.querySelectorAll(".model-row .model-name")].map((name) => name.textContent),
    ["whisper-base-en"],
  );
});

test("unconfigured files stay compact and can be adopted as local chat", async () => {
  const { dom, root } = await open();
  const row = root.querySelector(".orphan-row");
  assert.match(row.textContent, /stray-7B-Q5_K_S\.gguf/);
  assert.match(row.querySelector(".orphan-size").textContent, /4\.6\u00a0GiB/);
  row.querySelector(".orphan-adopt").click();
  await settle();
  assert.equal(dom.window.location.hash, "#/local/stray-7B-Q5_K_S");
  assert.equal(root.querySelector(".orphan-row"), null);
});

test("orphan deletion validates digests, confirms, and refreshes the list", async () => {
  const invalid = {
    path: "models/unverified.gguf",
    size_bytes: 42,
    sha256: "not-a-digest",
  };
  const marker = {
    path: "models/model.verified",
    size_bytes: 64,
    sha256: "b".repeat(64),
  };
  const { dom, root, stub } = await open(modelsFixture(), {
    orphans: [ORPHAN, invalid, marker],
  });
  const rows = [...root.querySelectorAll(".orphan-row")];
  assert.equal(rows.length, 2, "ArtifactStore marker files stay hidden");
  const disabled = rows.find((row) => row.textContent.includes("unverified.gguf"));
  assert.equal(disabled.querySelector(".orphan-delete").disabled, true);
  assert.match(disabled.querySelector(".disabled-tooltip").title, /verified digest/);

  rows.find((row) => row.textContent.includes("stray-7B")).querySelector(".orphan-delete").click();
  await settle();
  dom.window.document.querySelector(".confirm-overlay .button-danger").click();
  await settle();
  assert.ok(
    stub.calls.some(
      (call) =>
        call.init.method === "DELETE" &&
        call.url.endsWith(`/v1/cache/${ORPHAN.sha256}`),
    ),
  );
  assert.doesNotMatch(root.querySelector(".orphan-section")?.textContent ?? "", /stray-7B/);
});

test("local and STT detail panes show artifact status and deletion", async () => {
  const cache = [
    {
      source: "models/Qwen3-8B-Q4_K_M.gguf",
      path: "C:/pf/cache/models/Qwen3-8B-Q4_K_M.gguf",
      sha256: "c".repeat(64),
      size_bytes: 8 * 1024 ** 3,
    },
    {
      source: "models/ggml-base.en.bin",
      path: "C:/pf/cache/models/ggml-base.en.bin",
      sha256: "d".repeat(64),
      size_bytes: 150 * 1024 ** 2,
    },
  ];
  const { dom, root } = await open(modelsFixture(), { cache });
  navigate(dom, "#/local/qwen-common");
  await settle();
  assert.match(root.querySelector(".file-status").textContent, /Downloaded 8\.0\u00a0GiB/);
  assert.match(root.querySelector(".file-cache-path").textContent, /Qwen3-8B-Q4_K_M\.gguf$/);
  assert.ok(root.querySelector(".cached-delete"));
  root.querySelector(".cached-delete").click();
  await settle();
  dom.window.document.querySelector(".confirm-overlay .button-danger").click();
  await settle();
  assert.match(root.querySelector(".file-status").textContent, /Not downloaded/);

  navigate(dom, "#/local/whisper-base-en");
  await settle();
  assert.match(root.querySelector(".file-status").textContent, /Downloaded 150\u00a0MiB/);
  assert.ok(root.querySelector(".cached-delete"), "STT gets the same per-entry delete action");
});

test("deleting a profiled model names profiles and removes every reference atomically", async () => {
  const { dom, root, stub } = await open();
  navigate(dom, "#/local/qwen-common");
  await settle();
  root.querySelector(".detail-delete").click();
  await settle();
  const dialog = dom.window.document.querySelector(".confirm-overlay");
  assert.match(dialog.textContent, /default/, "the confirmation names the affected profile");
  dialog.querySelector(".button-danger").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  const body = JSON.parse(put.init.body);
  assert.equal(body.local_model.some((model) => model.name === "qwen-common"), false);
  assert.equal(
    body.profile.some((profile) => profile.models.includes("qwen-common")),
    false,
    "the same payload removes all dangling checklist references",
  );
});

test("canceling profiled model deletion leaves configuration untouched", async () => {
  const { dom, root, stub } = await open();
  navigate(dom, "#/local/qwen-common");
  await settle();
  root.querySelector(".detail-delete").click();
  await settle();
  dom.window.document.querySelector(".confirm-overlay .button-outline").click();
  await settle();
  assert.equal(
    stub.calls.some(
      (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
    ),
    false,
  );
  assert.ok(stub.state.pending.local_model.some((model) => model.name === "qwen-common"));
  assert.ok(
    stub.state.pending.profile.find((profile) => profile.name === "default").models.includes(
      "qwen-common",
    ),
  );
});
