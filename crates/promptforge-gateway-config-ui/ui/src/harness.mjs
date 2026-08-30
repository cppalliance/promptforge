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
 * leaf with flash attention off.
 */
export function modelsFixture() {
  const common = "C:/pf/profiles/common.toml";
  const leaf = "C:/pf/profiles/default.toml";
  return {
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

/**
 * A canned gateway behind the fetch signature: status, profiles, an
 * idle progress stream, switch-profile (overridable through `onSwitch`),
 * and the config surface - running/pending/dirty views, shadow saves
 * (PUT re-points the pending view and flips the dirty report), apply,
 * revert, orphans, model-info, reveal, and cache deletes. When `key` is
 * set, requests without that bearer answer 401. Every call is recorded
 * in `calls`; the mutable config state is exposed as `state`.
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
} = {}) {
  const calls = [];
  const state = {
    config: config ?? {},
    pending: pending ?? structuredClone(config ?? {}),
    dirty: dirty ?? cleanDirty(),
    orphans: orphans ?? [],
  };
  const fetchFn = async (input, init = {}) => {
    const url = String(input);
    calls.push({ url, init });
    if (key !== undefined) {
      const headers = init.headers ?? {};
      const auth = headers.Authorization ?? headers.authorization;
      if (auth !== `Bearer ${key}`) {
        return jsonResponse({ error: "unauthorized" }, 401);
      }
    }
    if (url.endsWith("/admin/status")) {
      return jsonResponse({ profile, models });
    }
    if (url.endsWith("/admin/profiles")) {
      return jsonResponse({ profiles });
    }
    if (url.endsWith("/admin/progress")) {
      return sseChannel().response;
    }
    if (url.endsWith("/admin/config-pending")) {
      return jsonResponse({ profile: state.pending, boot: null });
    }
    if (url.endsWith("/admin/config-dirty")) {
      return jsonResponse(state.dirty);
    }
    if (url.endsWith("/admin/config-apply")) {
      state.config = structuredClone(state.pending);
      state.dirty = cleanDirty();
      return jsonResponse({
        applied: ["profiles/default.toml"],
        reloaded: true,
        restart_required: false,
      });
    }
    if (url.endsWith("/admin/config-revert")) {
      state.pending = structuredClone(state.config);
      state.dirty = cleanDirty();
      return jsonResponse({ reverted: ["profiles/default.toml.next"] });
    }
    if (url.endsWith("/admin/config")) {
      if ((init.method ?? "GET") === "PUT") {
        state.pending = JSON.parse(init.body);
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
      return jsonResponse({});
    }
    if (url.endsWith("/admin/switch-profile")) {
      if (onSwitch) {
        return onSwitch(init);
      }
      const channel = sseChannel();
      channel.push({ status: "ready", profile: JSON.parse(init.body).name });
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
