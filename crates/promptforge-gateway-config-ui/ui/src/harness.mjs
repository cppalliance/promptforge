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

/**
 * A canned gateway behind the fetch signature: status, profiles, an
 * idle progress stream, and switch-profile (overridable through
 * `onSwitch`). When `key` is set, requests without that bearer answer
 * 401. Every call is recorded in `calls`.
 */
export function gatewayStub({ profile = "default", profiles = ["default"], key, onSwitch } = {}) {
  const calls = [];
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
      return jsonResponse({ profile, models: [] });
    }
    if (url.endsWith("/admin/profiles")) {
      return jsonResponse({ profiles });
    }
    if (url.endsWith("/admin/progress")) {
      return sseChannel().response;
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
  return { fetchFn, calls };
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
