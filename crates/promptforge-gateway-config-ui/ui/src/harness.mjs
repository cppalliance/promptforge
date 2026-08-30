// Shared jsdom harness for the live-shell tests. The bundle is imported
// once per test process (node --test runs each file in its own
// process); it reads the DOM globals at call time, so every test swaps
// in a fresh jsdom window and calls the exported `boot` with injected
// dependencies - a stub fetch standing in for the gateway, jsdom's
// window for location, sessionStorage, and hashchange.
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
 * A config fixture: a leaf profile (`default.toml`) including
 * `common.toml`, so provenance and inheritance render. One remote model
 * and one endpoint live in the leaf; one local model and the dominion
 * are inherited from the common file; a second local model lives in the
 * leaf with flash attention off. The top-level `include` array is the
 * leaf's own line, verbatim, as the gateway emits it.
 */
export function modelsFixture() {
  const common = "C:/pf/profiles/common.toml";
  const leaf = "C:/pf/profiles/default.toml";
  return {
    include: ["common.toml"],
    server: { bind: "127.0.0.1:8081", api_key: "***" },
    local: { cache_dir: "~/.promptforge" },
    dominion: [
      {
        id: "gpu0",
        kind: "local",
        max_queue: 100,
        policy: "queue",
        fair_scheduling: true,
        source_file: common,
      },
    ],
    endpoint: [
      {
        id: "openai",
        protocol: "openai",
        base_url: "https://api.openai.com/v1",
        api_key: "***",
        source_file: leaf,
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
        source_file: leaf,
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
        source_file: common,
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
        source_file: leaf,
      },
    ],
    source_files: { "server.bind": leaf },
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
    siblings: [
      { rfilename: "README.md", size: 1234 },
      { rfilename: "config.json", size: 99 },
      { rfilename: "Qwen3-Test-8B-Q4_K_M.gguf", size: 10 * GIB },
      { rfilename: "Qwen3-Test-8B-Q6_K.gguf", size: 18 * GIB },
      { rfilename: "Qwen3-Test-8B-Q8_0.gguf", size: 25 * GIB },
      { rfilename: "Qwen3-Test-8B-F16.gguf", size: 50 * GIB },
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

/** A README carrying the XSS vectors the sanitizer must neutralize. */
export function readmeFixture() {
  return [
    "# Qwen3 Test",
    "",
    "<script>window.__pwned = true;</script>",
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
 * A canned gateway behind the fetch signature: status, profiles, an
 * idle progress stream, switch-profile (overridable through `onSwitch`),
 * and the config surface - running/pending/dirty views, shadow saves
 * (PUT re-points the pending view, redacting secrets to "***" the way
 * the gateway's pending view does, and flips the dirty report), the
 * boot-config shadow save (PUT /admin/boot-config merges the body's boot
 * sections into the pending view and records `state.boot`), apply
 * (outcome overridable through `applyOutcome`), revert, orphans,
 * model-info, reveal, and cache deletes. Profile files:
 * `POST`/`DELETE /admin/profiles/{name}` against the mutable listing
 * (`state.profiles`; create answers 409 for an existing name, delete
 * 409 for the active profile), `PUT /admin/include/{path}` recorded in
 * `state.includes` (`onPutInclude` overrides the response), and
 * `onPutConfig` optionally staging a refusal for the profile shadow
 * save before the stub's own handling. The Discover
 * surface: `/admin/system` (`system`), the HF proxy (`hfSearch` rows
 * and `hfModels` by repo; `hfAuth401` makes both answer the hub's
 * pass-through 401), hub README fetches (`readme`), `POST
 * /v1/cache` (an immediately-completing SSE unless `onCache` supplies
 * the response), and the `GET /v1/cache` listing (`cache`; a DELETE
 * removes the matching entry; `onCacheList` overrides the listing
 * response, for failure staging). When `key` is set, gateway requests without that
 * bearer answer 401; absolute (hub) URLs are exempt. Every call is
 * recorded in `calls`; the mutable config state is exposed as `state`.
 */
export function gatewayStub({
  profile = "default",
  profiles = ["default"],
  models = [],
  key,
  onSwitch,
  config,
  pending,
  dirty,
  orphans,
  modelInfo,
  dirtyAfterSave,
  system,
  hfSearch,
  hfModels,
  hfAuth401 = false,
  readme,
  onCache,
  cache,
  onCacheList,
  applyOutcome,
  onPutConfig,
  onPutInclude,
} = {}) {
  const calls = [];
  const state = {
    config: config ?? {},
    pending: pending ?? structuredClone(config ?? {}),
    dirty: dirty ?? cleanDirty(),
    orphans: orphans ?? [],
    /** The GET /v1/cache listing; deletes remove matching entries. */
    cache: cache ?? [],
    /** The last PUT /admin/boot-config body, null until one arrives. */
    boot: null,
    /** The profile listing; creates append, deletes remove. */
    profiles: [...profiles],
    /** Include shadows by decoded path: the last PUT /admin/include body. */
    includes: {},
    /** The active profile; the default switch handler re-points it. */
    active: profile,
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
      const headers = init.headers ?? {};
      const auth = headers.Authorization ?? headers.authorization;
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
    if (url.endsWith("/v1/cache") && (init.method ?? "GET") === "GET") {
      if (onCacheList) {
        return onCacheList(init);
      }
      return jsonResponse(state.cache);
    }
    if (url.endsWith("/v1/cache") && init.method === "POST") {
      if (onCache) {
        return onCache(init);
      }
      const channel = sseChannel();
      channel.push({ status: "downloading", bytes: 10, total: 100 });
      channel.push({ status: "ready", path: "C:/pf/models/file.gguf" });
      channel.end();
      return channel.response;
    }
    if (url.endsWith("/admin/status")) {
      return jsonResponse({ profile: state.active, models });
    }
    // Profile files: POST creates (409 for an existing name, mirroring
    // the gateway), DELETE removes (409 for the active profile, 404 for
    // a missing one). The listing tracks both.
    if (url.includes("/admin/profiles/")) {
      const name = decodeURIComponent(url.slice(url.lastIndexOf("/") + 1));
      if (init.method === "POST") {
        if (state.profiles.includes(name)) {
          return jsonResponse(
            { error: { code: "profile_exists", message: `profile ${name} already exists` } },
            409,
          );
        }
        state.profiles.push(name);
        return jsonResponse({ created: `profiles/${name}.toml` });
      }
      if (init.method === "DELETE") {
        if (name === state.active) {
          return jsonResponse(
            {
              error: {
                code: "profile_active",
                message: `profile ${name} is active and cannot be deleted`,
              },
            },
            409,
          );
        }
        if (!state.profiles.includes(name)) {
          return jsonResponse(
            { error: { code: "profile_not_found", message: `no profile ${name}` } },
            404,
          );
        }
        state.profiles = state.profiles.filter((entry) => entry !== name);
        return jsonResponse({ deleted: `profiles/${name}.toml`, shadow_removed: false });
      }
    }
    if (url.endsWith("/admin/profiles")) {
      return jsonResponse({ profiles: state.profiles });
    }
    // The include-file shadow save; `onPutInclude` stages refusals.
    if (url.includes("/admin/include/") && init.method === "PUT") {
      const path = decodeURIComponent(
        url.slice(url.indexOf("/admin/include/") + "/admin/include/".length),
      );
      if (onPutInclude) {
        return onPutInclude(init, path);
      }
      state.includes[path] = JSON.parse(init.body);
      return jsonResponse({ shadow: `profiles/${path}.next` });
    }
    if (url.endsWith("/admin/progress")) {
      return sseChannel().response;
    }
    if (url.endsWith("/admin/config-pending")) {
      return jsonResponse({
        profile: state.pending,
        boot:
          state.boot === null
            ? null
            : { shadow: "gateway.toml.next", changed_sections: Object.keys(state.boot) },
      });
    }
    if (url.endsWith("/admin/config-dirty")) {
      return jsonResponse(state.dirty);
    }
    if (url.endsWith("/admin/boot-config") && init.method === "PUT") {
      state.boot = JSON.parse(init.body);
      // The pending profile view resolves boot shadows too, so the boot
      // sections it renders track the staged edit.
      for (const section of ["server", "workshop"]) {
        if (section in state.boot) {
          state.pending[section] = structuredClone(state.boot[section]);
        }
      }
      redactSecrets(state.pending);
      state.dirty = {
        dirty: true,
        pending_files: ["gateway.toml"],
        changed_sections: Object.keys(state.boot),
      };
      return jsonResponse({ shadow: "gateway.toml.next" });
    }
    if (url.endsWith("/admin/config-apply")) {
      state.config = structuredClone(state.pending);
      state.dirty = cleanDirty();
      state.boot = null;
      return jsonResponse(
        applyOutcome ?? {
          applied: ["profiles/default.toml"],
          reloaded: true,
          restart_required: false,
        },
      );
    }
    if (url.endsWith("/admin/config-revert")) {
      state.pending = structuredClone(state.config);
      state.dirty = cleanDirty();
      return jsonResponse({ reverted: ["profiles/default.toml.next"] });
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
        // The gateway grafts the leaf's include line onto a candidate
        // that lacks one, so the chain keeps visiting the boot file:
        // boot-owned sections the body omits survive in the pending
        // view (staged boot edits included).
        for (const section of ["server", "workshop"]) {
          if (!(section in body) && section in state.pending) {
            body[section] = structuredClone(state.pending[section]);
          }
        }
        state.pending = body;
        redactSecrets(state.pending);
        state.dirty = dirtyAfterSave ?? {
          dirty: true,
          pending_files: ["profiles/default.toml"],
          changed_sections: [],
        };
        return jsonResponse({ shadow: "profiles/default.toml.next" });
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
    if (url.endsWith("/admin/switch-profile")) {
      if (onSwitch) {
        return onSwitch(init);
      }
      const channel = sseChannel();
      state.active = JSON.parse(init.body).name;
      channel.push({ status: "ready", profile: state.active });
      channel.end();
      return channel.response;
    }
    return jsonResponse({ error: `unstubbed route: ${url}` }, 404);
  };
  return { fetchFn, calls, state };
}

/**
 * Boots the app into a fresh jsdom: optionally seeds the stored key,
 * mounts a container, and calls the bundle's `boot` with the stub's
 * fetch. Returns the dom and the mounted root.
 */
export async function bootApp({ url = CONFIG_URL, key, stub } = {}) {
  const app = await loadApp();
  const dom = makeDom(url);
  if (key !== undefined) {
    dom.window.sessionStorage.setItem(app.API_KEY_STORAGE_KEY, key);
  }
  const root = dom.window.document.createElement("div");
  dom.window.document.body.append(root);
  app.boot(root, { win: dom.window, fetchFn: stub?.fetchFn });
  await settle();
  return { dom, root };
}

/** Sets the hash and fires hashchange synchronously for the router. */
export function navigate(dom, hash) {
  dom.window.location.hash = hash;
  dom.window.dispatchEvent(new dom.window.HashChangeEvent("hashchange"));
}
