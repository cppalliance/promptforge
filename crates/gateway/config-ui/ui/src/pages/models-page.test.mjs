import assert from "node:assert/strict";
import test from "node:test";

import { bundledDeclarations, squash } from "../css-support.mjs";
import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../test-support.mjs";

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

test("STT entries show the Mic badge and implicit non-editable kind", async () => {
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

test("editing an STT model and applying defers the restart verdict to the gateway", async () => {
  const { dom, root } = await open();
  navigate(dom, "#/local/whisper-base-en");
  await settle();
  const source = root.querySelector(".field-row[data-key='source'] input");
  source.value = "models/ggml-large-v3.bin";
  source.dispatchEvent(new dom.window.Event("change"));
  await settle();
  root.querySelector(".detail-save").click();
  await settle();

  root.querySelector(".apply-button").click();
  await settle();

  const toasts = [...root.querySelectorAll(".toast")].map((toast) => toast.textContent);
  assert.ok(
    !toasts.some((text) => /speech-to-text/.test(text)),
    "the browser raises no speech toast of its own; restart_required from the apply decides",
  );
});

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("the search box reads Add or search model and a search with no match says No models available", async () => {
  const { dom, root } = await open();
  const input = root.querySelector("#models-search");
  assert.equal(input.placeholder, "Add or search model");
  assert.equal(root.querySelector(".model-list-empty"), null, "rows exist, so no empty note");

  input.value = "no-such-model";
  input.dispatchEvent(new dom.window.Event("input"));
  await sleep(250);
  const empty = root.querySelector(".model-list-empty");
  assert.ok(empty, "an empty result renders the empty state");
  assert.equal(empty.textContent, "No models available");
  assert.equal(root.querySelectorAll(".model-row").length, 0);
});

/** Boots the app with a gateway whose `/admin/config` reload waits until `release()`. */
async function openWithHeldReload() {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), orphans: [ORPHAN] });
  const real = stub.fetchFn;
  let hold = false;
  let release = () => undefined;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  stub.fetchFn = async (url, init) => {
    if (hold && String(url).endsWith("/admin/config")) {
      await gate;
    }
    return real(url, init);
  };
  const { dom, root } = await bootApp({ key: "k", stub });
  await settle();
  return {
    dom,
    root,
    hold: () => {
      hold = true;
    },
    release,
  };
}

test("the refresh icon spins while the catalog reloads and stops once it lands", async () => {
  const { root, hold, release } = await openWithHeldReload();

  const refresh = root.querySelector(".models-refresh");
  assert.equal(refresh.getAttribute("aria-label"), "Refresh models");
  assert.ok(refresh.querySelector("svg"), "the button is an icon");
  assert.equal(refresh.classList.contains("is-loading"), false, "idle until pressed");

  hold();
  refresh.click();
  await settle();
  const spinning = root.querySelector(".models-refresh");
  assert.ok(spinning.classList.contains("is-loading"), "spins while the reload is in flight");
  assert.equal(spinning.getAttribute("aria-busy"), "true");

  release();
  await settle(20);
  assert.equal(
    root.querySelector(".models-refresh").classList.contains("is-loading"),
    false,
    "stops when the load settles",
  );
});

test("pressing refresh from the keyboard keeps focus on the refresh button through both renders", async () => {
  const { dom, root, hold, release } = await openWithHeldReload();
  const doc = dom.window.document;

  const refresh = root.querySelector(".models-refresh");
  refresh.focus();
  assert.equal(doc.activeElement, refresh, "the press starts with focus on the button");

  hold();
  refresh.click();
  await settle();
  const spinning = root.querySelector(".models-refresh");
  assert.notEqual(spinning, refresh, "the toolbar was rebuilt, so the old button is gone");
  assert.equal(doc.activeElement, spinning, "focus returns to the new button while the reload runs");

  release();
  await settle(20);
  const settled = root.querySelector(".models-refresh");
  assert.equal(settled.classList.contains("is-loading"), false, "the reload landed");
  assert.equal(doc.activeElement, settled, "focus is on the final button after the load settles");
});

test("a reload does not pull focus onto refresh when the press did not leave it there", async () => {
  const { dom, root, hold, release } = await openWithHeldReload();
  const doc = dom.window.document;

  hold();
  root.querySelector(".models-refresh").click();
  await settle();
  assert.notEqual(doc.activeElement, root.querySelector(".models-refresh"), "unfocused press, no focus taken");

  // Focus moves to the search box while the reload is in flight; the landing render leaves it alone.
  const search = root.querySelector("#models-search");
  search.focus();
  release();
  await settle(20);
  assert.notEqual(
    doc.activeElement,
    root.querySelector(".models-refresh"),
    "the settled reload does not steal focus",
  );
});

test("a non-numeric entry in a model's numeric field raises an alert under the input and stages nothing", async () => {
  const { dom, root, stub } = await open();
  navigate(dom, "#/local/qwen-common");
  await settle();

  const row = root.querySelector(".field-row[data-key='max_output']");
  const input = row.querySelector("input");
  input.value = "lots";
  input.dispatchEvent(new dom.window.Event("change"));

  const error = row.querySelector(".field-error");
  assert.ok(error, "the bad entry raises a field error");
  assert.equal(error.getAttribute("role"), "alert");
  assert.equal(error.textContent, "Enter a number");
  assert.equal(input.getAttribute("aria-invalid"), "true");
  assert.ok(
    input.getAttribute("aria-describedby").split(" ").includes(error.id),
    "the input is described by the alert",
  );
  assert.equal(input.nextElementSibling, error, "the alert sits right under the input");
  assert.equal(
    stub.calls.filter((call) => call.init?.method === "PUT").length,
    0,
    "nothing was staged",
  );

  // A good number commits an edit, and the page re-render takes the alert with the old input.
  const typed = root.querySelector(".field-row[data-key='max_output'] input");
  typed.value = "512";
  typed.dispatchEvent(new dom.window.Event("change"));
  await settle();
  const fixed = root.querySelector(".field-row[data-key='max_output']");
  assert.equal(fixed.querySelector(".field-error"), null, "a valid number leaves no alert");
  assert.equal(fixed.querySelector("input").getAttribute("aria-invalid"), null);
  assert.equal(fixed.querySelector("input").value, "512", "the edit is kept");
});

test("model rows sit 12px apart with no hover fill, and the search box takes Cursor's padding", async () => {
  const list = await bundledDeclarations(".model-list");
  assert.equal(list.get("display"), "flex");
  assert.equal(list.get("flex-direction"), "column");
  assert.equal(list.get("gap"), "var(--space-3)", "a 12px gap");
  const hover = await bundledDeclarations(".model-row:hover");
  assert.equal(hover.get("background"), undefined, "no hover fill");
  const search = await bundledDeclarations(".models-toolbar .input");
  assert.equal(squash(search.get("padding")), "5px12px", "padding 5px 12px");
  assert.equal(search.get("border-radius"), "var(--radius-sm)", "a 4px radius");
  const spin = await bundledDeclarations(".models-refresh.is-loading svg");
  assert.match(spin.get("animation") ?? "", /spin/, "the icon rotates while loading");
});
