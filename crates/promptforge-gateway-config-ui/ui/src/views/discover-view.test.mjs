// Pins the Discover view: the 300ms-debounced search (keywords hit the
// search proxy once; user/repo and pasted URLs hit the model endpoint),
// the sort-to-hub-parameter mapping, the quant table with exact sizes
// and heuristic fit badges plus the starred Recommended row, README
// sanitization through marked, the no-HF_TOKEN banner, and the
// download path through the global store into the top progress strip.
import assert from "node:assert/strict";
import test from "node:test";

import {
  GIB,
  bootApp,
  gatewayStub,
  hfModelFixture,
  hfSearchFixture,
  navigate,
  readmeFixture,
  settle,
  sseChannel,
  systemFixture,
} from "../harness.mjs";

const REPO = "unsloth/Qwen3-Test-8B-GGUF";

function discoverStub(extra = {}) {
  return gatewayStub({
    key: "k",
    hfSearch: hfSearchFixture(),
    hfModels: { [REPO]: hfModelFixture() },
    readme: readmeFixture(),
    system: systemFixture(),
    ...extra,
  });
}

/** The recorded calls that hit the HF search proxy. */
function searchCalls(stub) {
  return stub.calls.filter((call) => call.url.includes("/admin/hf/search"));
}

/** The recorded calls that hit the HF model proxy. */
function modelCalls(stub) {
  return stub.calls.filter((call) => call.url.includes("/admin/hf/model/"));
}

/** Boots to #/discover and lets the initial browse search finish. */
async function openDiscover(stub) {
  const booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/discover");
  await settle();
  return booted;
}

/** Types into the debounced search box without waiting the debounce out. */
function type(root, text) {
  const input = root.querySelector("#discover-search");
  input.value = text;
  input.dispatchEvent(new input.ownerDocument.defaultView.Event("input"));
}

/** Waits `ms` of real time (the debounce runs on real timers). */
function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

test("the search debounces to one proxied call and renders result rows", async () => {
  const stub = discoverStub();
  const { root } = await openDiscover(stub);
  assert.equal(searchCalls(stub).length, 1, "mount runs one initial browse search");

  type(root, "q");
  type(root, "qw");
  type(root, "qwen");
  await sleep(150);
  assert.equal(searchCalls(stub).length, 1, "no call fires inside the debounce window");
  await sleep(250);
  await settle();
  const calls = searchCalls(stub);
  assert.equal(calls.length, 2, "three keystrokes collapse into one search");
  assert.match(calls[1].url, /q=qwen/, "the settled text is the query");
  assert.match(calls[1].url, /filter=gguf/, "the GGUF filter is pinned");

  const rows = [...root.querySelectorAll(".result-row")];
  assert.equal(rows.length, 2, "every fixture repo renders a row");
  assert.equal(rows[0].querySelector(".model-name").textContent, REPO);
  assert.ok(rows[0].querySelector(".result-avatar"), "the row carries the publisher avatar");
  assert.equal(rows[0].querySelector(".result-params").textContent, "8B");
  assert.match(rows[0].textContent, /1\.2M/, "downloads render compact");
  assert.match(rows[0].textContent, /3d ago/, "the updated time renders relative");
});

test("user/repo and pasted URL forms hit the model endpoint, not search", async () => {
  const stub = discoverStub();
  const { root } = await openDiscover(stub);
  const searchBaseline = searchCalls(stub).length;

  type(root, REPO);
  await sleep(350);
  await settle();
  assert.equal(modelCalls(stub).length, 1, "a user/repo form hits the model endpoint");
  assert.match(modelCalls(stub)[0].url, /\/admin\/hf\/model\/unsloth\/Qwen3-Test-8B-GGUF$/);
  assert.equal(searchCalls(stub).length, searchBaseline, "no search call fires for a repo form");
  assert.equal(
    root.querySelector(".hub-detail-title")?.textContent,
    "Qwen3-Test-8B-GGUF",
    "the detail card opens directly",
  );

  type(root, `https://huggingface.co/${REPO}`);
  await sleep(350);
  await settle();
  assert.equal(modelCalls(stub).length, 2, "a pasted hub URL also hits the model endpoint");
});

test("the sort options map to the hub's sort parameters", async () => {
  const stub = discoverStub();
  const { root } = await openDiscover(stub);
  assert.match(searchCalls(stub)[0].url, /sort=downloads/, "the default sort is downloads");
  assert.match(searchCalls(stub)[0].url, /direction=-1/, "descending direction is pinned");

  const sort = root.querySelector("#discover-sort");
  const change = () => sort.dispatchEvent(new sort.ownerDocument.defaultView.Event("change"));
  sort.value = "trending";
  change();
  await settle();
  assert.match(searchCalls(stub).at(-1).url, /sort=trendingScore/, "trending maps to trendingScore");

  root.querySelector("#discover-sort").value = "newest";
  root
    .querySelector("#discover-sort")
    .dispatchEvent(new sort.ownerDocument.defaultView.Event("change"));
  await settle();
  assert.match(searchCalls(stub).at(-1).url, /sort=lastModified/, "newest maps to lastModified");
});

test("the quant table renders exact sizes, heuristic fit badges, and one Recommended star", async () => {
  const stub = discoverStub();
  const { root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  const rows = [...root.querySelectorAll(".quant-table tbody tr")];
  assert.deepEqual(
    rows.map((row) => row.dataset.quant),
    ["Q4_K_M", "Q6_K", "Q8_0", "F16"],
    "only GGUF siblings enter the table, smallest first",
  );
  assert.deepEqual(
    rows.map((row) => row.querySelector(".quant-size").textContent),
    ["10 GiB", "18 GiB", "25 GiB", "50 GiB"],
    "sizes come from the blobs=true sibling list",
  );
  // 20 GiB free VRAM / 24 total / 32 GiB free RAM, sizes weighted 1.2:
  // 12 fits free VRAM, 21.6 fits total VRAM only, 30 fits free RAM
  // only, 60 fits nothing.
  assert.deepEqual(
    rows.map((row) => row.querySelector(".fit-badge").dataset.fit),
    ["gpu", "partial", "cpu", "none"],
    "the fit badges follow the heuristic against /admin/system",
  );
  assert.deepEqual(
    rows.map((row) => row.querySelector(".fit-badge").textContent),
    ["Fits GPU", "Partial offload", "CPU only", "Too large"],
  );

  const recommended = root.querySelectorAll(".quant-table .is-recommended");
  assert.equal(recommended.length, 1, "exactly one row is starred");
  assert.equal(
    recommended[0].dataset.quant,
    "Q4_K_M",
    "the largest quant that fully fits free VRAM is Recommended",
  );
  assert.match(recommended[0].textContent, /Recommended/);
});

test("the README renders through marked with raw HTML and unsafe URLs neutralized", async () => {
  const stub = discoverStub();
  const { dom, root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  const markdown = root.querySelector(".readme .markdown");
  assert.ok(markdown, "the README renders below the detail card");
  assert.equal(markdown.querySelector("script"), null, "a script tag never becomes an element");
  assert.equal(dom.window.__pwned, undefined, "the script payload never ran");
  assert.match(
    markdown.textContent,
    /window\.__pwned/,
    "the raw HTML is escaped into visible text",
  );
  assert.ok(markdown.querySelector("strong"), "ordinary markdown still renders");
  const badAnchor = [...markdown.querySelectorAll("a")].find((anchor) =>
    (anchor.getAttribute("href") ?? "").startsWith("javascript:"),
  );
  assert.equal(badAnchor, undefined, "a javascript: link loses its href");
  assert.match(markdown.textContent, /bad link/, "the unsafe link's text survives");
});

test("a javascript: scheme hidden behind HTML entities is still stripped", async () => {
  // The browser decodes entities in an href before reading the scheme,
  // so `&#106;avascript:` and `javascript&colon;` reconstitute into a
  // live javascript: URL unless the sanitizer decodes before checking.
  const readme = [
    "[num entity](&#106;avascript:alert(1))",
    "",
    "[hex entity](&#x6A;avascript:alert(2))",
    "",
    "[named entity](javascript&colon;alert(3))",
    "",
    "[newline](java&#10;script:alert(4))",
  ].join("\n");
  const stub = discoverStub({ readme });
  const { root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  const markdown = root.querySelector(".readme .markdown");
  for (const anchor of markdown.querySelectorAll("a")) {
    const raw = anchor.getAttribute("href") ?? "";
    const decoded = raw
      .replace(/&#x([0-9a-f]+);?/gi, (_, hex) => String.fromCodePoint(parseInt(hex, 16)))
      .replace(/&#(\d+);?/g, (_, dec) => String.fromCodePoint(parseInt(dec, 10)))
      .replace(/&colon;/gi, ":")
      .replace(/[\u0000-\u0020]/g, "");
    assert.doesNotMatch(decoded, /^javascript:/i, `href ${raw} decodes to a script URL`);
  }
});

test("with no GPU the fit heuristic falls back to the RAM bands without crashing", async () => {
  const stub = discoverStub({
    system: { ram: { used_bytes: 32 * GIB, total_bytes: 64 * GIB }, gpu: null },
  });
  const { root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  const badges = [...root.querySelectorAll(".quant-table tbody .fit-badge")];
  assert.deepEqual(
    badges.map((badge) => badge.dataset.fit),
    ["cpu", "cpu", "cpu", "none"],
    "without a GPU every quant is CPU-only until free RAM is exceeded",
  );
  assert.equal(
    root.querySelectorAll(".quant-table .is-recommended").length,
    0,
    "no GPU means no Recommended star",
  );
});

test("a hub 401 (no HF_TOKEN) shows the Secrets banner instead of the key prompt", async () => {
  const stub = discoverStub({ hfAuth401: true });
  const { root } = await openDiscover(stub);

  const banner = root.querySelector(".banner-token");
  assert.ok(banner, "the no-token banner mounts");
  assert.match(banner.textContent, /Set HF_TOKEN in Secrets to enable Hugging Face search/);
  assert.equal(banner.querySelector("a")?.getAttribute("href"), "#/secrets");
  assert.ok(
    root.querySelector(".tab-bar"),
    "the shell stays mounted: a hub 401 must not clear the gateway key",
  );
});

test("Download starts the cache SSE, the store feeds the strip, and completion clears it", async () => {
  let channel;
  const stub = discoverStub({
    onCache: () => {
      channel = sseChannel();
      return channel.response;
    },
  });
  const { dom, root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  root.querySelector(".quant-download").click();
  await settle();

  const post = stub.calls.find(
    (call) => call.url.endsWith("/v1/cache") && call.init.method === "POST",
  );
  assert.ok(post, "the click POSTs /v1/cache");
  assert.equal(
    JSON.parse(post.init.body).source,
    `https://huggingface.co/${REPO}/resolve/main/Qwen3-Test-8B-Q4_K_M.gguf`,
    "the source is the quant file's hub resolve URL",
  );
  assert.match(
    dom.window.document.querySelector(".toast")?.textContent ?? "",
    /Download started/,
    "a toast confirms the start",
  );

  const strip = root.querySelector(".global-progress");
  assert.ok(strip, "the shell mounts the top progress strip");
  assert.equal(strip.hidden, false, "an active download reveals the strip");
  const button = root.querySelector(".quant-download");
  assert.equal(button.disabled, true, "the active quant's button disables");
  assert.match(button.textContent, /Downloading/);

  channel.push({ status: "downloading", bytes: 5 * GIB, total: 10 * GIB });
  await settle();
  assert.equal(
    strip.querySelector(".progress-strip-bar").style.getPropertyValue("--progress"),
    "0.5",
    "the strip tracks the store's fraction",
  );

  channel.push({ status: "ready", path: "C:/pf/models/Qwen3-Test-8B-Q4_K_M.gguf" });
  channel.end();
  await settle();
  assert.equal(strip.hidden, true, "completion clears the strip");
  assert.equal(
    root.querySelector(".quant-download").disabled,
    false,
    "the button re-enables after completion",
  );
});
