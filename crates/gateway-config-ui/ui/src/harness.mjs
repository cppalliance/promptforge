// Shared jsdom harness for the live-shell tests. The bundle is imported
// once per test process (node --test runs each file in its own
// process); it reads the DOM globals at call time, so every test swaps
// in a fresh jsdom window and calls the exported `boot` with injected
// dependencies - a stub fetch standing in for the gateway, jsdom's
// window for location, sessionStorage, and hashchange.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { JSDOM } from "jsdom";

const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "dist");

const BLANK_PAGE = "<!DOCTYPE html><html><body></body></html>";
const CONFIG_URL = "http://127.0.0.1:8081/config/";

let appModulePromise;

/** Imports the built bundle once; run `npm run build` (or a debug `cargo build`) first. */
export function loadApp() {
  if (!appModulePromise) {
    // Module evaluation touches `document` (the #app auto-boot probe),
    // so a throwaway DOM must exist before the first import.
    if (!globalThis.document) {
      makeDom();
    }
    appModulePromise = import(pathToFileURL(path.join(distDir, "app.js")).href);
  }
  return appModulePromise;
}

/** Fresh jsdom window installed as the global DOM. */
export function makeDom(url = CONFIG_URL) {
  const dom = new JSDOM(BLANK_PAGE, { url });
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  return dom;
}

/** Lets queued microtasks and zero-delay timers run. */
export async function settle(turns = 10) {
  for (let i = 0; i < turns; i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

/** A JSON response the API wrapper can consume. */
export function jsonResponse(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

/** An SSE response with push/end controls for staged event delivery. */
export function sseChannel() {
  let controller;
  const encoder = new TextEncoder();
  const stream = new ReadableStream({
    start(c) {
      controller = c;
    },
  });
  return {
    response: new Response(stream, {
      status: 200,
      headers: { "content-type": "text/event-stream" },
    }),
    push(event) {
      controller.enqueue(encoder.encode(`data: ${JSON.stringify(event)}\n\n`));
    },
    /** Raw bytes, for splitting one event across chunk boundaries. */
    pushRaw(text) {
      controller.enqueue(encoder.encode(text));
    },
    end() {
      controller.close();
    },
  };
}

/** An empty dirty report: no shadow files exist. */
function cleanDirty() {
  return { dirty: false, pending_files: [], changed_sections: [] };
}

/**
 * A single-file config fixture with remote, local chat, STT, and two
 * profile checklists.
 */
export function modelsFixture() {
  return {
    "config-version": 2,
    active_profile: "default",
    server: { bind: "127.0.0.1:8081", api_key: "***" },
    local: { cache_dir: "~/.promptforge" },
    dominion: [
      {
        id: "gpu0",
        kind: "local",
        max_queue: 100,
        policy: "queue",
        fair_scheduling: true,
      },
    ],
    endpoint: [
      {
        id: "openai",
        protocol: "openai",
        base_url: "https://api.openai.com/v1",
        api_key: "***",
      },
    ],
    model: [
      {
        name: "gpt-remote",
        kind: "chat",
        description: "remote chat model",
        context: 128000,
        thinking: "switchable",
        upstream: "gpt-4.1",
        endpoints: ["openai"],
        default_max_tokens: null,
        tool_dialect: "openai",
        images: false,
        parallel_tool_calls: true,
        effort_levels: ["low", "high"],
        default_effort: "low",
        adaptive_thinking: false,
      },
    ],
    local_model: [
      {
        name: "qwen-common",
        kind: "chat",
        description: "inherited from common",
        source: "models/Qwen3-8B-Q4_K_M.gguf",
        sha256: null,
        dominion: "gpu0",
        parallel: 1,
        vram_gb: 8,
        context: 8192,
        thinking: "switchable",
        gpu_layers: 99,
        flash_attention: true,
        cache_type_k: "q8_0",
        cache_type_v: "q4_0",
        n_predict: 8192,
        chat_template_file: null,
        speculative: null,
        multimodal_projector: null,
        images: false,
        parallel_tool_calls: false,
        effort_levels: [],
        adaptive_thinking: false,
      },
      {
        name: "llama-leaf",
        kind: "chat",
        description: "defined in the leaf",
        source: "models/Llama-3-8B-Q8_0.gguf",
        sha256: null,
        dominion: null,
        parallel: 1,
        vram_gb: null,
        context: 4096,
        thinking: "never",
        gpu_layers: 40,
        flash_attention: false,
        cache_type_k: "q8_0",
        cache_type_v: "q4_0",
        n_predict: 8192,
        chat_template_file: null,
        speculative: null,
        multimodal_projector: null,
        images: false,
        parallel_tool_calls: false,
        effort_levels: [],
        adaptive_thinking: false,
      },
    ],
    stt_model: [
      {
        name: "whisper-base-en",
        role: "interim",
        source: "models/ggml-base.en.bin",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        vram_gb: 1,
        dominion: "gpu0",
      },
    ],
    profile: [
      { name: "default", models: ["gpt-remote", "qwen-common", "whisper-base-en"] },
      { name: "travel", models: ["llama-leaf"] },
    ],
  };
}

/**
 * A `GET /admin/env` reply: the global env file with values, plus the
 * server-computed `${VAR}` references from the pre-interpolation config.
 */
export function envFixture() {
  return {
    boot: {
      path: "C:/pf/gateway.env",
      vars: {
        GATEWAY_KEY: "boot-master-key",
        HF_TOKEN: "hf-fixture-token",
        OPENAI_KEY: "sk-fixture",
      },
    },
    references: { OPENAI_KEY: ["endpoint openai api_key"] },
  };
}

/** A GiB, for HF fixture sizes. */
export const GIB = 1024 ** 3;

/**
 * A system snapshot for the fit heuristic: 20 GiB free VRAM of 24,
 * 32 GiB free RAM of 64. With the plan's 1.2 margin: 10 GiB fits the
 * GPU, 18 GiB partially offloads, 25 GiB is CPU only, 50 GiB is too
 * large.
 */
export function systemFixture() {
  return {
    cpu: { frequency_mhz: 2500, logical_cores: 16, physical_cores: 8, utilization_percent: 5 },
    ram: { used_bytes: 32 * GIB, total_bytes: 64 * GIB },
    disk: { cache_dir: "C:/pf/cache", used_bytes: 700 * GIB, total_bytes: 4096 * GIB },
    gpu: { name: "NVIDIA GeForce RTX 4090", vram_used_bytes: 4 * GIB, vram_total_bytes: 24 * GIB },
  };
}

/** Hub search results: two GGUF repos as the hub's /api/models emits them. */
export function hfSearchFixture() {
  return [
    {
      id: "unsloth/Qwen3-Test-8B-GGUF",
      downloads: 1_234_567,
      likes: 890,
      lastModified: new Date(Date.now() - 3 * 86_400_000).toISOString(),
      tags: ["gguf", "qwen3"],
    },
    {
      id: "bartowski/Llama-X-GGUF",
      downloads: 45_000,
      likes: 12,
      lastModified: new Date(Date.now() - 40 * 86_400_000).toISOString(),
      tags: ["gguf"],
    },
  ];
}

/**
 * Hub model detail with blobs=true sizes chosen to hit every fit band
 * against `systemFixture`, plus non-GGUF siblings the picker must skip.
 */
export function hfModelFixture() {
  return {
    id: "unsloth/Qwen3-Test-8B-GGUF",
    author: "unsloth",
    downloads: 1_234_567,
    likes: 890,
    lastModified: new Date(Date.now() - 3 * 86_400_000).toISOString(),
    tags: ["gguf", "qwen3", "text-generation"],
    pipeline_tag: "text-generation",
    siblings: [
      { rfilename: "README.md", size: 1234 },
      { rfilename: "config.json", size: 99 },
      {
        rfilename: "Qwen3-Test-8B-Q4_K_M.gguf",
        size: 10 * GIB,
        lfs: { sha256: "1".repeat(64) },
      },
      {
        rfilename: "Qwen3-Test-8B-Q6_K.gguf",
        size: 18 * GIB,
        lfs: { sha256: "2".repeat(64) },
      },
      {
        rfilename: "Qwen3-Test-8B-Q8_0.gguf",
        size: 25 * GIB,
        lfs: { sha256: "3".repeat(64) },
      },
      {
        rfilename: "Qwen3-Test-8B-F16.gguf",
        size: 50 * GIB,
        lfs: { sha256: "4".repeat(64) },
      },
    ],
  };
}

/**
 * A cache listing as `GET /v1/cache` emits it: source URL, absolute
 * blob path, sha256, and size - the sidecars carry no timestamp.
 */
export function cacheListFixture() {
  return [
    {
      source: "https://huggingface.co/u/r/resolve/main/Qwen3-8B-Q4_K_M.gguf",
      path: "C:/pf/cache/models/Qwen3-8B-Q4_K_M.gguf",
      sha256: "a".repeat(64),
      size_bytes: 10 * GIB,
    },
    {
      source: "https://huggingface.co/u/r/resolve/main/Llama-3-8B-Q8_0.gguf",
      path: "C:/pf/cache/models/Llama-3-8B-Q8_0.gguf",
      sha256: "b".repeat(64),
      size_bytes: 8 * GIB,
    },
  ];
}

/** The server-owned chat-template catalog and effective model decisions. */
export function chatTemplateCatalogFixture() {
  return {
    families: [
      { slug: "chatml", label: "ChatML" },
      { slug: "llama-3", label: "Llama 3" },
      { slug: "llama-3.1", label: "Llama 3.1" },
      { slug: "qwen-2.5", label: "Qwen 2.5" },
      { slug: "qwen-3", label: "Qwen 3" },
      { slug: "gemma-3", label: "Gemma 3" },
      { slug: "gemma-4", label: "Gemma 4" },
      { slug: "mistral", label: "Mistral" },
      { slug: "phi-3", label: "Phi 3" },
      { slug: "phi-4", label: "Phi 4" },
      { slug: "gpt-oss", label: "GPT OSS" },
      { slug: "zephyr", label: "Zephyr" },
    ],
    mappings: [
      { model_id: "qwen/qwen3-8b", family: "qwen-3" },
      { model_id: "meta-llama/meta-llama-3-8b-instruct", family: "llama-3" },
    ],
    models: [
      {
        name: "qwen-common",
        effective_source: "embedded",
        effective_family: null,
        detected_family: "qwen-3",
        reason: "Auto uses the GGUF embedded template.",
      },
      {
        name: "llama-leaf",
        effective_source: "embedded",
        effective_family: null,
        detected_family: "llama-3",
        reason: "Auto uses the GGUF embedded template.",
      },
    ],
  };
}

/** A README carrying the XSS vectors the sanitizer must neutralize. */
export function readmeFixture() {
  return [
    "---",
    "license: apache-2.0",
    "tags:",
    "  - gguf",
    "---",
    "",
    "# Qwen3 Test",
    "",
    "<script>window.__pwned = true;</script>",
    "",
    '<p class="safe-inline">Inline <em>HTML</em> survives.</p>',
    "",
    '<img src="x" onerror="window.__pwned = true" alt="unsafe event">',
    "",
    "Safe **body** text.",
    "",
    "[bad link](javascript:alert(1))",
  ].join("\n");
}

/**
 * Mirrors the gateway's pending-view secret redaction: every secret
 * field serializes as "***" no matter what a PUT staged, so a typed key
 * can never echo back into the UI from the stub either.
 */
function redactSecrets(view) {
  if (view.server && typeof view.server === "object" && typeof view.server.api_key === "string") {
    view.server.api_key = "***";
  }
  for (const entry of Array.isArray(view.endpoint) ? view.endpoint : []) {
    if (typeof entry.api_key === "string") {
      entry.api_key = "***";
    }
  }
  const webSearch =
    view.tools && typeof view.tools === "object" ? view.tools.web_search : undefined;
  if (webSearch && typeof webSearch.api_key === "string") {
    webSearch.api_key = "***";
  }
}

/**
 * A canned gateway behind the fetch signature: status, an idle progress
 * stream, and the config surface - running/pending/dirty views, shadow saves
 * (PUT re-points the pending view, redacting secrets to "***" the way
 * the gateway's pending view does, and flips the dirty report), apply
 * (outcome overridable through `applyOutcome`), revert, orphans,
 * model-info, reveal, and cache deletes. `onPutConfig` optionally stages
 * a refusal before the stub's own handling. The Discover
 * surface: `/admin/system` (`system`), the HF proxy (`hfSearch` rows
 * and `hfModels` by repo; `hfAuth401` makes both answer the hub's
 * pass-through 401), hub README fetches (`readme`), and the `GET
 * /v1/cache` listing (`cache`; a DELETE
 * removes the matching entry; `onCacheList` overrides the listing
 * response, for failure staging). The env surface: `GET /admin/env`
 * returns `env` (both sides null when unstubbed) and `PUT /admin/env`
 * records `{ scope, vars }` in `state.envPuts` and marks the dirty
 * report. When `key` is set, gateway requests without that
 * bearer answer 401; absolute (hub) URLs are exempt. Every call is
 * recorded in `calls`; the mutable config state is exposed as `state`.
 * The queue surface: `/admin/status` returns `queue`, `endpoints`, and
 * `vram_gb` from `state` (mutate them to drive the status bar), and the
 * cancel routes record into `state.cancelActiveCalls` and
 * `state.cancelPendingCalls`.
 */
export function gatewayStub({
  profile = "default",
  configGeneration = "generation-1",
  configGenerationAfterApply,
  models = [],
  key,
  config,
  pending,
  dirty,
  orphans,
  modelInfo,
  chatTemplates,
  dirtyAfterSave,
  system,
  hfSearch,
  hfModels,
  hfAuth401 = false,
  readme,
  cache,
  onCacheList,
  applyOutcome,
  onPutConfig,
  env,
  queue,
  endpoints,
  vramGb,
} = {}) {
  const calls = [];
  const state = {
    config: config ?? {},
    pending: pending ?? structuredClone(config ?? {}),
    dirty: dirty ?? cleanDirty(),
    orphans: orphans ?? [],
    /** The GET /v1/cache listing; deletes remove matching entries. */
    cache: cache ?? [],
    /** The GET /admin/env reply (both sides null when unstubbed). */
    env: env ?? { boot: null, references: {} },
    /** Every PUT /admin/env, as `{ scope, vars }` in arrival order. */
    envPuts: [],
    /** The active profile; the default switch handler re-points it. */
    active: profile,
    /** Process-lifetime config generation returned by admin status. */
    configGeneration,
    /** The command queue readout returned by admin status. */
    queue: queue ?? { active: null, pending: [] },
    /** The endpoint readiness entries returned by admin status. */
    endpoints: endpoints ?? [],
    /** The declared VRAM total returned by admin status. */
    vramGb: vramGb ?? 0,
    /** Count of POST /admin/queue/cancel calls received. */
    cancelActiveCalls: 0,
    /** Every POST /admin/queue/cancel-pending body, in arrival order. */
    cancelPendingCalls: [],
  };
  const hubDenied = () =>
    jsonResponse(
      {
        error: {
          message: "upstream returned 401",
          type: "invalid_request_error",
          code: "upstream_client_error",
        },
      },
      401,
    );
  const fetchFn = async (input, init = {}) => {
    const url = String(input);
    calls.push({ url, init });
    // Hub-served files (README, avatars) carry no gateway bearer.
    if (/^https?:\/\//.test(url)) {
      if (url.endsWith("README.md")) {
        return new Response(readme ?? "# hello", {
          status: 200,
          headers: { "content-type": "text/markdown" },
        });
      }
      return jsonResponse({ error: `unstubbed hub route: ${url}` }, 404);
    }
    if (key !== undefined) {
      const headers = new Headers(init.headers);
      const auth = headers.get("Authorization");
      if (auth !== `Bearer ${key}`) {
        return jsonResponse({ error: "unauthorized" }, 401);
      }
    }
    if (url.includes("/admin/hf/search")) {
      if (hfAuth401) {
        return hubDenied();
      }
      return jsonResponse(hfSearch ?? []);
    }
    if (url.includes("/admin/hf/model/") && url.endsWith("/readme")) {
      if (hfAuth401) {
        return hubDenied();
      }
      if (readme === null) {
        return new Response("", { status: 404 });
      }
      return new Response(readme ?? "# hello", {
        status: 200,
        headers: { "content-type": "text/markdown; charset=utf-8" },
      });
    }
    if (url.includes("/admin/hf/model/")) {
      if (hfAuth401) {
        return hubDenied();
      }
      const repo = url.slice(url.indexOf("/admin/hf/model/") + "/admin/hf/model/".length);
      const detail = (hfModels ?? {})[repo];
      return detail
        ? jsonResponse(detail)
        : jsonResponse({ error: { code: "upstream_client_error" } }, 404);
    }
    if (url.endsWith("/admin/system")) {
      return jsonResponse(system ?? systemFixture());
    }
    if (url.endsWith("/admin/chat-templates")) {
      // An explicit null simulates a headless gateway without the route.
      if (chatTemplates === null) {
        return jsonResponse({ error: "not found" }, 404);
      }
      return jsonResponse(chatTemplates ?? chatTemplateCatalogFixture());
    }
    if (url.endsWith("/v1/cache") && (init.method ?? "GET") === "GET") {
      if (onCacheList) {
        return onCacheList(init);
      }
      return jsonResponse(state.cache);
    }
    if (url.endsWith("/admin/status")) {
      return jsonResponse({
        profile: state.active,
        models,
        config_generation: state.configGeneration,
        vram_gb: state.vramGb,
        queue: state.queue,
        endpoints: state.endpoints,
      });
    }
    if (url.endsWith("/admin/queue/cancel-pending")) {
      state.cancelPendingCalls.push(JSON.parse(init.body ?? "{}"));
      return jsonResponse({ cancelled: true });
    }
    if (url.endsWith("/admin/queue/cancel")) {
      state.cancelActiveCalls += 1;
      return jsonResponse({ cancelled: state.queue.active !== null });
    }
    if (url.endsWith("/admin/progress")) {
      return sseChannel().response;
    }
    // The env surface: GET returns the real files; PUT stages a shadow
    // (recorded, never merged back - the real file is untouched) and
    // flips the dirty report the way a staged env shadow does.
    if (url.includes("/admin/env")) {
      if (init.method === "PUT") {
        const scope = "global";
        state.envPuts.push({ scope, vars: JSON.parse(init.body) });
        const file = "gateway.env";
        state.dirty = {
          dirty: true,
          pending_files: [...new Set([...state.dirty.pending_files, file])],
          changed_sections: state.dirty.changed_sections,
        };
        return jsonResponse({ shadow: `${file}.next` });
      }
      return jsonResponse(state.env);
    }
    if (url.endsWith("/admin/config-pending")) {
      return jsonResponse({
        profile: state.pending,
        boot: null,
      });
    }
    if (url.endsWith("/admin/config-dirty")) {
      return jsonResponse(state.dirty);
    }
    if (url.endsWith("/admin/config-apply")) {
      state.config = structuredClone(state.pending);
      if (typeof state.pending.active_profile === "string") {
        state.active = state.pending.active_profile;
      }
      state.dirty = cleanDirty();
      if (configGenerationAfterApply !== undefined) {
        state.configGeneration = configGenerationAfterApply;
      }
      return jsonResponse(
        applyOutcome ?? {
          applied: ["gateway.toml"],
          reloaded: true,
          restart_required: false,
        },
      );
    }
    if (url.endsWith("/admin/config-revert")) {
      state.pending = structuredClone(state.config);
      state.dirty = cleanDirty();
      return jsonResponse({ reverted: ["gateway.toml.next"] });
    }
    if (url.endsWith("/admin/config")) {
      if ((init.method ?? "GET") === "PUT") {
        if (onPutConfig) {
          const staged = onPutConfig(init);
          if (staged) {
            return staged;
          }
        }
        const body = JSON.parse(init.body);
        // Boot-owned sections the body omits survive in the pending view.
        for (const section of ["server", "workshop"]) {
          if (!(section in body) && section in state.pending) {
            body[section] = structuredClone(state.pending[section]);
          }
        }
        state.pending = body;
        redactSecrets(state.pending);
        state.dirty = dirtyAfterSave ?? {
          dirty: true,
          pending_files: ["gateway.toml"],
          changed_sections: [],
        };
        return jsonResponse({ shadow: "gateway.toml.next" });
      }
      return jsonResponse(state.config);
    }
    if (url.endsWith("/admin/orphans")) {
      return jsonResponse({ orphans: state.orphans });
    }
    if (url.includes("/admin/model-info")) {
      return modelInfo
        ? jsonResponse(modelInfo)
        : jsonResponse({ error: "not a gguf" }, 422);
    }
    if (url.endsWith("/admin/reveal")) {
      return jsonResponse({});
    }
    if (url.includes("/v1/cache/") && init.method === "DELETE") {
      const sha = url.slice(url.lastIndexOf("/") + 1);
      state.orphans = state.orphans.filter((orphan) => orphan.sha256 !== sha);
      state.cache = state.cache.filter((entry) => entry.sha256 !== sha);
      return jsonResponse({});
    }
    return jsonResponse({ error: `unstubbed route: ${url}` }, 404);
  };
  return { fetchFn, calls, state };
}

/**
 * Boots the app into a fresh jsdom: optionally seeds the stored key,
 * mounts a container, and calls the bundle's `boot` with the stub's
 * fetch plus any extra boot options (the panel-mode bridge seams).
 * Returns the dom and the mounted root.
 */
export async function bootApp({ url = CONFIG_URL, key, stub, options } = {}) {
  const app = await loadApp();
  const dom = makeDom(url);
  if (key !== undefined) {
    dom.window.sessionStorage.setItem(app.API_KEY_STORAGE_KEY, key);
  }
  const root = dom.window.document.createElement("div");
  dom.window.document.body.append(root);
  app.boot(root, { win: dom.window, fetchFn: stub?.fetchFn, ...(options ?? {}) });
  await settle();
  return { dom, root };
}

let displayRulesPromise;

/**
 * Every `display` declaration in the built dist/app.css, in cascade
 * order: `@layer` blocks flattened in their declared order, unlayered
 * rules last (they beat every layer), source order within. Rules under
 * a media condition are skipped: jsdom has no viewport, so the
 * mobile-first base styles are the ones a narrow window would apply.
 */
function loadDisplayRules() {
  if (!displayRulesPromise) {
    displayRulesPromise = (async () => {
      const css = await readFile(path.join(distDir, "app.css"), "utf8");
      const scratch = new JSDOM(BLANK_PAGE);
      const style = scratch.window.document.createElement("style");
      style.textContent = css;
      scratch.window.document.head.append(style);
      const layerOrder = [];
      const layered = new Map();
      const unlayered = [];
      for (const rule of style.sheet.cssRules) {
        if (rule.constructor.name === "CSSLayerStatementRule") {
          layerOrder.push(...rule.nameList);
        } else if (rule.constructor.name === "CSSLayerBlockRule") {
          const list = layered.get(rule.name) ?? [];
          layered.set(rule.name, list);
          list.push(...rule.cssRules);
        } else {
          unlayered.push(rule);
        }
      }
      // The model covers normal declarations in statement-ordered layers
      // only; anything outside that (an unlisted layer, an !important
      // display) would be silently mis-ranked, so refuse it instead.
      for (const name of layered.keys()) {
        if (!layerOrder.includes(name)) {
          throw new Error(`bundledDisplay: layer "${name}" is not in the @layer statement`);
        }
      }
      const ordered = [...layerOrder.flatMap((name) => layered.get(name) ?? []), ...unlayered];
      return ordered.flatMap((rule) => {
        const display = rule.style?.getPropertyValue("display");
        if (!display) {
          return [];
        }
        if (rule.style.getPropertyPriority("display") === "important") {
          throw new Error(`bundledDisplay: "${rule.selectorText}" sets display !important`);
        }
        return [{ selector: rule.selectorText, display }];
      });
    })();
  }
  return displayRulesPromise;
}

/**
 * Specificity (ids, classes-attributes-pseudo-classes, types) of one
 * complex selector. Covers the constructs the sheets use; `:not()` and
 * `:is()` count their argument, which is exact for a single argument.
 */
function specificity(selector) {
  let ids = 0;
  let classes = 0;
  let types = 0;
  selector
    .replace(/::[\w-]+/g, () => {
      types += 1;
      return " ";
    })
    .replace(/:(?:not|is|where|has)\(/g, "(")
    .replace(/\[[^\]]*\]/g, () => {
      classes += 1;
      return " ";
    })
    .replace(/#[\w-]+/g, () => {
      ids += 1;
      return " ";
    })
    .replace(/\.[\w-]+/g, () => {
      classes += 1;
      return " ";
    })
    .replace(/:[\w-]+(?:\([^)]*\))?/g, () => {
      classes += 1;
      return " ";
    })
    .replace(/(?<![\w-])[a-zA-Z][\w-]*/g, () => {
      types += 1;
      return " ";
    });
  return [ids, classes, types];
}

/** Splits a selector list on its top-level commas. */
function selectorParts(selectorText) {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < selectorText.length; i += 1) {
    const ch = selectorText[i];
    if (ch === "(" || ch === "[") {
      depth += 1;
    } else if (ch === ")" || ch === "]") {
      depth -= 1;
    } else if (ch === "," && depth === 0) {
      parts.push(selectorText.slice(start, i).trim());
      start = i + 1;
    }
  }
  parts.push(selectorText.slice(start).trim());
  return parts;
}

function matchesPart(element, part) {
  // Pseudo-element selectors never match the element itself; anything
  // else jsdom cannot parse must surface rather than drop the rule.
  return part.includes("::") ? false : element.matches(part);
}

/**
 * The `display` a browser would compute for the element from the built
 * stylesheet: the winning author declaration by cascade order, else the
 * UA default (`none` for a `hidden` element, `block` otherwise).
 *
 * jsdom's own getComputedStyle cannot answer this: it skips `@layer`
 * blocks, and its UA `[hidden]` rule outranks author classes by raw
 * specificity, so it reports `none` for a hidden element whose class
 * sets an explicit `display` - exactly the case a browser renders.
 */
export async function bundledDisplay(element) {
  const rules = await loadDisplayRules();
  let winner = null;
  for (const rule of rules) {
    const matching = selectorParts(rule.selector).filter((part) => matchesPart(element, part));
    if (matching.length === 0) {
      continue;
    }
    const best = matching
      .map(specificity)
      .reduce((a, b) => (compareSpecificity(a, b) >= 0 ? a : b));
    if (winner === null || compareSpecificity(best, winner.specificity) >= 0) {
      winner = { specificity: best, display: rule.display };
    }
  }
  if (winner !== null) {
    return winner.display;
  }
  return element.hidden ? "none" : "block";
}

function compareSpecificity(a, b) {
  for (let i = 0; i < 3; i += 1) {
    if (a[i] !== b[i]) {
      return a[i] - b[i];
    }
  }
  return 0;
}

/** Sets the hash and fires hashchange synchronously for the router. */
export function navigate(dom, hash) {
  dom.window.location.hash = hash;
  dom.window.dispatchEvent(new dom.window.HashChangeEvent("hashchange"));
}
