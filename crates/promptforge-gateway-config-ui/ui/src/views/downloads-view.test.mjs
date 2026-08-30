// Pins the Downloads view: active cards driven by the download store
// (percent, speed, ETA following the cache SSE stream; a failed entry
// shows its error and Retry restarts through the store), completed rows
// from GET /v1/cache with confirm-then-DELETE-then-refresh, the
// listing-failure banner with its Retry, both empty states, and the
// Downloads tab's active-count badge.
import assert from "node:assert/strict";
import test from "node:test";

import {
  GIB,
  bootApp,
  cacheListFixture,
  gatewayStub,
  hfModelFixture,
  hfSearchFixture,
  jsonResponse,
  navigate,
  settle,
  sseChannel,
  systemFixture,
} from "../harness.mjs";

const REPO = "unsloth/Qwen3-Test-8B-GGUF";

function downloadsStub(extra = {}) {
  return gatewayStub({
    key: "k",
    hfSearch: hfSearchFixture(),
    hfModels: { [REPO]: hfModelFixture() },
    system: systemFixture(),
    ...extra,
  });
}

/** The recorded GET /v1/cache listing calls. */
function cacheListCalls(stub) {
  return stub.calls.filter(
    (call) => call.url.endsWith("/v1/cache") && (call.init.method ?? "GET") === "GET",
  );
}

/** The recorded POST /v1/cache download starts. */
function cachePosts(stub) {
  return stub.calls.filter(
    (call) => call.url.endsWith("/v1/cache") && call.init.method === "POST",
  );
}

/**
 * Boots the app and starts the fixture's smallest quant download
 * through the Discover view, the store's only production entry point.
 */
async function startDownload(stub) {
  const booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/discover");
  await settle();
  booted.root.querySelector(".result-row").click();
  await settle();
  booted.root.querySelector(".quant-download").click();
  await settle();
  return booted;
}

/** Real-time sleep, so the store's speed sampling sees time pass. */
function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

test("an active card renders percent, speed, and ETA and follows the stream", async () => {
  const channels = [];
  const stub = downloadsStub({
    onCache: () => {
      const channel = sseChannel();
      channels.push(channel);
      return channel.response;
    },
  });
  const { dom, root } = await startDownload(stub);
  navigate(dom, "#/downloads");
  await settle();

  const card = root.querySelector(".downloads-active .download-card");
  assert.ok(card, "the in-flight store entry renders a card");
  assert.equal(
    card.querySelector(".download-name").textContent,
    "Qwen3-Test-8B-Q4_K_M.gguf",
    "the card names the file from the source URL",
  );
  assert.equal(card.querySelector(".download-retry"), null, "no cancel/retry on a live download");

  // No Content-Length: bytes downloaded stand in for the percent, and
  // the progressbar stays indeterminate (no aria-valuenow).
  channels[0].push({ status: "downloading", bytes: 1.5 * GIB, total: null });
  await settle();
  const unknown = root.querySelector(".downloads-active .download-card");
  assert.equal(
    unknown.querySelector(".download-percent").textContent,
    "1.5 GiB downloaded",
    "an unknown total falls back to bytes downloaded",
  );
  assert.equal(
    unknown.querySelector(".progress-bar").getAttribute("aria-valuenow"),
    null,
    "no aria-valuenow while the total is unknown",
  );

  await sleep(20);
  channels[0].push({ status: "downloading", bytes: 2.5 * GIB, total: 10 * GIB });
  await settle();
  const updated = root.querySelector(".downloads-active .download-card");
  assert.equal(updated.querySelector(".download-percent").textContent, "25%");
  assert.equal(
    updated.querySelector(".progress-bar-fill").style.getPropertyValue("--progress"),
    "0.25",
    "the lava bar scales by the store's fraction",
  );
  assert.match(updated.querySelector(".download-speed").textContent, /\/s$/);
  assert.match(updated.querySelector(".download-eta").textContent, /^ETA /);

  await sleep(20);
  channels[0].push({ status: "downloading", bytes: 5 * GIB, total: 10 * GIB });
  await settle();
  assert.equal(
    root.querySelector(".download-percent").textContent,
    "50%",
    "the card tracks the stream as it progresses",
  );

  const listBaseline = cacheListCalls(stub).length;
  channels[0].push({ status: "ready", path: "C:/pf/cache/models/Qwen3-Test-8B-Q4_K_M.gguf" });
  channels[0].end();
  await settle();
  assert.equal(
    root.querySelector(".downloads-active .download-card"),
    null,
    "a finished download leaves the Active section",
  );
  assert.match(
    root.querySelector(".downloads-active .view-empty").textContent,
    /No active downloads/,
  );
  assert.ok(
    cacheListCalls(stub).length > listBaseline,
    "completion refreshes the Completed listing",
  );
});

test("a failed entry shows its error and Retry restarts through the store", async () => {
  const channels = [];
  const stub = downloadsStub({
    onCache: () => {
      const channel = sseChannel();
      channels.push(channel);
      return channel.response;
    },
  });
  const { dom, root } = await startDownload(stub);
  channels[0].push({ status: "error", message: "disk full" });
  channels[0].end();
  navigate(dom, "#/downloads");
  await settle();

  const card = root.querySelector(".downloads-active .download-card");
  assert.ok(card, "the failed entry stays visible");
  assert.match(card.querySelector(".download-error").textContent, /disk full/);

  const baseline = cachePosts(stub).length;
  card.querySelector(".download-retry").click();
  await settle();
  assert.equal(cachePosts(stub).length, baseline + 1, "Retry POSTs /v1/cache again");
  assert.equal(
    JSON.parse(cachePosts(stub).at(-1).init.body).source,
    `https://huggingface.co/${REPO}/resolve/main/Qwen3-Test-8B-Q4_K_M.gguf`,
    "the retry restarts the same source",
  );
  assert.ok(
    root.querySelector(".downloads-active .progress-bar"),
    "the entry is downloading again",
  );
  channels[1].end();
  await settle();
});

test("completed rows render from GET /v1/cache and Delete confirms, deletes, refreshes", async () => {
  const stub = downloadsStub({ cache: cacheListFixture() });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/downloads");
  await settle();

  const rows = [...root.querySelectorAll(".completed-row")];
  assert.equal(rows.length, 2, "every cache entry renders a row");
  assert.equal(rows[0].querySelector(".completed-name").textContent, "Qwen3-8B-Q4_K_M.gguf");
  assert.equal(rows[0].querySelector(".completed-size").textContent, "10 GiB");
  assert.ok(rows[0].querySelector(".check-icon svg"), "a check icon leads the row");

  rows[0].querySelector(".completed-delete").click();
  await settle();
  const modal = dom.window.document.querySelector(".confirm-overlay .modal");
  assert.ok(modal, "Delete opens the confirm dialog first");
  assert.match(
    modal.textContent,
    /Qwen3-8B-Q4_K_M\.gguf \(10 GiB\)/,
    "the dialog names the file and its size",
  );
  assert.equal(
    stub.calls.filter((call) => call.init.method === "DELETE").length,
    0,
    "nothing is deleted before the confirmation",
  );
  modal.querySelector(".button-danger").click();
  await settle();

  const del = stub.calls.find(
    (call) => call.init.method === "DELETE" && call.url.includes("/v1/cache/"),
  );
  assert.ok(del, "confirming calls DELETE /v1/cache/{sha256}");
  assert.ok(del.url.endsWith("a".repeat(64)), "the delete names the row's digest");
  assert.equal(
    root.querySelectorAll(".completed-row").length,
    1,
    "the listing refreshes after the delete",
  );
});

test("a failed cache listing shows the error banner and Retry recovers", async () => {
  let fail = true;
  const stub = downloadsStub({
    onCacheList: () =>
      fail ? jsonResponse({ error: "cache unreadable" }, 500) : jsonResponse(cacheListFixture()),
  });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/downloads");
  await settle();

  const banner = root.querySelector(".downloads-completed .banner-danger");
  assert.ok(banner, "a failed listing renders the error banner");
  assert.match(banner.textContent, /The cache listing failed:/);
  assert.equal(root.querySelector(".completed-row"), null, "no rows render from a failed listing");

  fail = false;
  banner.querySelector("button").click();
  await settle();
  assert.equal(
    root.querySelector(".downloads-completed .banner-danger"),
    null,
    "a successful Retry clears the banner",
  );
  assert.equal(
    root.querySelectorAll(".completed-row").length,
    2,
    "Retry reloads the listing",
  );
});

test("the Downloads tab badge shows the active count and clears at zero", async () => {
  const channels = [];
  const stub = downloadsStub({
    onCache: () => {
      const channel = sseChannel();
      channels.push(channel);
      return channel.response;
    },
  });
  const { root } = await startDownload(stub);
  const badge = root.querySelector(".tab-badge");
  assert.ok(badge, "an active download raises the badge");
  assert.match(badge.textContent, /^1/, "the badge carries the active count");
  assert.ok(badge.closest('a[href="#/downloads"]'), "the badge sits on the Downloads tab");

  channels[0].push({ status: "ready", path: "C:/pf/cache/models/x.gguf" });
  channels[0].end();
  await settle();
  assert.equal(root.querySelector(".tab-badge"), null, "zero active downloads clears the badge");
});

test("both sections show empty states when nothing is active or cached", async () => {
  const stub = downloadsStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/downloads");
  await settle();
  assert.match(
    root.querySelector(".downloads-active .view-empty").textContent,
    /No active downloads/,
  );
  assert.match(
    root.querySelector(".downloads-completed .view-empty").textContent,
    /No completed downloads/,
  );
});
