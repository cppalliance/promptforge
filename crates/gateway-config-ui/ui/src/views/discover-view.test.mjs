// Pins the Discover view: the 300ms-debounced search (keywords hit the
// search proxy once; user/repo and pasted URLs hit the model endpoint),
// the sort-to-hub-parameter mapping, the quant table with exact sizes
// and heuristic fit badges plus the starred Recommended row, sanitized
// inline-HTML README rendering, the no-HF_TOKEN banner, and
// download-on-apply staging through the config store.
import assert from "node:assert/strict";
import test from "node:test";

import {
  GIB,
  bootApp,
  chatTemplateCatalogFixture,
  gatewayStub,
  hfModelFixture,
  hfSearchFixture,
  modelsFixture,
  navigate,
  readmeFixture,
  settle,
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

/** The recorded calls that hit the HF model detail proxy (not readme). */
function modelCalls(stub) {
  return stub.calls.filter(
    (call) => call.url.includes("/admin/hf/model/") && !call.url.endsWith("/readme"),
  );
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
  assert.match(
    calls[1].url,
    /pipeline_tag=text-generation/,
    "Chat is the default workload filter",
  );

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

  root.querySelector("#discover-sort").click();
  root.querySelector(".discover-toolbar .menu-item[data-value='trending']").click();
  await settle();
  assert.match(searchCalls(stub).at(-1).url, /sort=trendingScore/, "trending maps to trendingScore");

  root.querySelector("#discover-sort").click();
  root.querySelector(".discover-toolbar .menu-item[data-value='newest']").click();
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
    ["10\u00a0GiB", "18\u00a0GiB", "25\u00a0GiB", "50\u00a0GiB"],
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

test("the README keeps safe inline HTML and sanitizes XSS after rendering", async () => {
  const stub = discoverStub();
  const { dom, root } = await openDiscover(stub);
  dom.window.HTMLElement.prototype.setHTML = function setHTML(html) {
    this.dataset.nativeSanitizer = "used";
    this.innerHTML = html;
  };
  root.querySelector(".result-row").click();
  await settle();

  const markdown = root.querySelector(".readme .markdown");
  assert.ok(markdown, "the README renders below the detail card");
  assert.equal(markdown.querySelector("script"), null, "a script tag never becomes an element");
  assert.equal(dom.window.__pwned, undefined, "the script payload never ran");
  assert.doesNotMatch(markdown.textContent, /window\.__pwned/, "script content is removed");
  assert.ok(markdown.querySelector("strong"), "ordinary markdown still renders");
  assert.ok(markdown.querySelector(".safe-inline em"), "safe inline HTML survives");
  assert.equal(
    markdown.querySelector("img")?.hasAttribute("onerror"),
    false,
    "event-handler attributes are removed",
  );
  assert.equal(markdown.dataset.nativeSanitizer, "used", "the native Sanitizer sink is detected");
  assert.doesNotMatch(markdown.textContent, /apache-2\.0/, "frontmatter is stripped");
  const badAnchor = [...markdown.querySelectorAll("a")].find((anchor) =>
    (anchor.getAttribute("href") ?? "").startsWith("javascript:"),
  );
  assert.equal(badAnchor, undefined, "a javascript: link loses its href");
  assert.match(markdown.textContent, /bad link/, "the unsafe link's text survives");
});

test("README frontmatter cannot select gray-matter's JavaScript engine", async () => {
  const marker = "__promptforgeFrontmatterExecuted";
  delete globalThis[marker];
  const stub = discoverStub({
    readme: [
      "---javascript",
      `({ value: (globalThis.${marker} = true) })`,
      "---",
      "# Safe model card",
    ].join("\n"),
  });
  try {
    const { root } = await openDiscover(stub);
    root.querySelector(".result-row").click();
    await settle();
    assert.equal(globalThis[marker], undefined, "frontmatter is stripped without evaluation");
    assert.match(root.querySelector(".markdown")?.textContent ?? "", /Safe model card/);
  } finally {
    delete globalThis[marker];
  }
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

test("malformed hub search JSON is an error instead of an empty result", async () => {
  const stub = discoverStub({ hfSearch: { unexpected: true } });
  const { root } = await openDiscover(stub);
  assert.match(root.querySelector(".banner-danger")?.textContent ?? "", /invalid search JSON/);
});

test("Download stages a pending model without touching the cache", async () => {
  const stub = discoverStub();
  const { dom, root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  root.querySelector(".quant-download").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.ok(put, "the click stages the config shadow");
  const entry = JSON.parse(put.init.body).local_model[0];
  assert.equal(
    entry.source,
    `https://huggingface.co/${REPO}/resolve/main/Qwen3-Test-8B-Q4_K_M.gguf`,
    "the pending entry carries the hub resolve URL",
  );
  assert.equal(entry.sha256, "1".repeat(64), "the pending entry carries the LFS digest");
  assert.equal(entry.vram_gb, 10, "the pending entry carries the listing size as VRAM");
  assert.equal(entry.kind, "chat");
  assert.match(
    dom.window.document.querySelector(".toast")?.textContent ?? "",
    /Apply to download/,
    "the toast explains that Apply owns the transfer",
  );
  assert.equal(
    stub.calls.some((call) => call.url.endsWith("/v1/cache") && call.init.method === "POST"),
    false,
    "staging touches no artifact endpoint",
  );
  assert.equal(
    root.querySelector(".global-progress"),
    null,
    "the removed client download strip cannot get stuck",
  );
  assert.match(root.querySelector(".apply-button")?.textContent ?? "", /Apply \(1\)/);
  assert.equal(root.querySelector(".quant-download").textContent, "Added");
});

test("Download prefills a mapped built-in template from the server catalog", async () => {
  const catalog = chatTemplateCatalogFixture();
  catalog.mappings = [{ model_id: REPO.toLowerCase(), family: "qwen-3" }];
  const config = modelsFixture();
  const stub = discoverStub({
    config,
    pending: structuredClone(config),
    chatTemplates: catalog,
  });
  const { root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();
  root.querySelector(".quant-download").click();
  await settle();

  const staged = stub.state.pending.local_model.find((entry) =>
    String(entry.source).startsWith("https://huggingface.co/"),
  );
  assert.equal(
    staged.chat_template_file,
    "builtin:qwen-3",
    "the browser copies the current server mapper result, not a TypeScript model table",
  );
});

test("concurrent Download staging preserves every model and profile choice", async () => {
  const config = modelsFixture();
  const stub = discoverStub({ config, pending: structuredClone(config) });
  const fetch = stub.fetchFn;
  let releaseFirst;
  const firstGate = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  let configPuts = 0;
  stub.fetchFn = async (input, init = {}) => {
    if (String(input).endsWith("/admin/config") && init.method === "PUT") {
      configPuts += 1;
      if (configPuts === 1) {
        await firstGate;
      }
    }
    return fetch(input, init);
  };

  const { root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();
  root.querySelectorAll(".quant-download")[0].click();
  await settle();
  root.querySelectorAll(".quant-download")[1].click();
  await settle();
  assert.equal(configPuts, 1, "the second full-config write waits for the first refresh");

  releaseFirst();
  await settle(6);
  const staged = stub.state.pending.local_model.filter((entry) =>
    String(entry.source).startsWith("https://huggingface.co/"),
  );
  assert.equal(staged.length, 2, "both selected quants survive in the pending catalog");
  assert.ok(
    staged.every((entry) =>
      stub.state.pending.profile
        .find((profile) => profile.name === "default")
        .models.includes(entry.name),
    ),
    "the pending active profile chooses each staged model so Apply provisions it",
  );
});

test("workload toggles fan out pipeline tags and merge them as OR filters", async () => {
  const stub = discoverStub();
  const { root } = await openDiscover(stub);
  root.querySelector(".discover-type[data-type='embedding']").click();
  root.querySelector(".discover-type[data-type='stt']").click();
  await settle();

  const urls = searchCalls(stub)
    .slice(-4)
    .map((call) => new URL(call.url, "http://gateway.test"));
  assert.ok(urls.every((url) => url.searchParams.get("filter") === "gguf"));
  assert.deepEqual(
    urls.map((url) => url.searchParams.getAll("pipeline_tag")),
    [
      ["text-generation"],
      ["feature-extraction"],
      ["sentence-similarity"],
      ["automatic-speech-recognition"],
    ],
    "each upstream request carries one tag because Hugging Face intersects repeats",
  );
  assert.equal(
    root.querySelectorAll(".result-row").length,
    2,
    "repos returned by more than one workload request are deduplicated",
  );
});

test("leaving Discover aborts in-flight searches and cannot repaint the route", async () => {
  const stub = discoverStub();
  const fetch = stub.fetchFn;
  let blockSearch = false;
  let aborted = false;
  stub.fetchFn = async (input, init = {}) => {
    if (blockSearch && String(input).includes("/admin/hf/search")) {
      return new Promise((_resolve, reject) => {
        init.signal.addEventListener("abort", () => {
          aborted = true;
          reject(new DOMException("aborted", "AbortError"));
        });
      });
    }
    return fetch(input, init);
  };
  const { dom, root } = await openDiscover(stub);
  blockSearch = true;
  type(root, "pending");
  await sleep(350);
  navigate(dom, "#/settings");
  await settle();

  assert.equal(aborted, true);
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Settings");
});

test("an STT-filtered Download stages a first-class stt_model entry", async () => {
  const detail = hfModelFixture();
  detail.tags = ["gguf"];
  detail.pipeline_tag = "automatic-speech-recognition";
  const config = modelsFixture();
  const stub = discoverStub({
    config,
    pending: structuredClone(config),
    hfModels: { [REPO]: detail },
  });
  const { root } = await openDiscover(stub);
  root.querySelector(".discover-type[data-type='chat']").click();
  root.querySelector(".discover-type[data-type='stt']").click();
  await settle();
  root.querySelector(".result-row").click();
  await settle();
  root.querySelector(".quant-download").click();
  await settle();

  const staged = stub.state.pending.stt_model.find((entry) =>
    String(entry.source).includes("Q4_K_M"),
  );
  assert.equal(staged.role, "interim");
  assert.equal(staged.kind, undefined, "STT kind comes from its catalog array");
  assert.ok(
    stub.state.pending.profile
      .find((profile) => profile.name === "default")
      .models.includes(staged.name),
    "Apply will provision the newly chosen STT artifact",
  );
});

test("the README is fetched through the gateway proxy, not directly from the hub", async () => {
  const stub = discoverStub();
  const { root } = await openDiscover(stub);
  root.querySelector(".result-row").click();
  await settle();

  const readmeCalls = stub.calls.filter(
    (call) => call.url.includes("/admin/hf/model/") && call.url.endsWith("/readme"),
  );
  assert.equal(readmeCalls.length, 1, "one readme call goes through the proxy");
  const hubReadmeCalls = stub.calls.filter(
    (call) => /^https?:\/\//.test(call.url) && call.url.includes("README.md"),
  );
  assert.equal(hubReadmeCalls.length, 0, "no direct hub URL calls for the README");
  assert.ok(root.querySelector(".readme .markdown"), "the README still renders");
});

test("an STT toggle does not misclassify another workload result", async () => {
  const detail = hfModelFixture();
  detail.tags = ["gguf", "automatic-speech-recognition"];
  detail.pipeline_tag = "feature-extraction";
  const config = modelsFixture();
  const stub = discoverStub({
    config,
    pending: structuredClone(config),
    hfModels: { [REPO]: detail },
  });
  const { root } = await openDiscover(stub);
  root.querySelector(".discover-type[data-type='chat']").click();
  root.querySelector(".discover-type[data-type='embedding']").click();
  root.querySelector(".discover-type[data-type='stt']").click();
  await settle();
  root.querySelector(".result-row").click();
  await settle();
  root.querySelector(".quant-download").click();
  await settle();

  assert.ok(
    stub.state.pending.local_model.some((entry) => String(entry.source).includes("Q4_K_M")),
  );
  assert.equal(
    stub.state.pending.stt_model.some((entry) => String(entry.source).includes("Q4_K_M")),
    false,
  );
});
