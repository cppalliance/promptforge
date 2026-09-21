// Shared browser-environment fixture for the bundle-level workshop tests:
// the jsdom boot that smoke.mjs originally built inline, extracted so every
// per-feature slice boots the exact same way. bootWorkbench(name, run)
// loads dist/index.html into jsdom, stands in fakes for the APIs jsdom
// lacks (WebSocket, audio capture, fetch, layout metrics), imports the built
// bundle (which lazy-loads its feature chunks from dist/chunks/),
// waits for the app to settle, then runs `run` under the shared
// disposable-leak check and reports the verdict through the
// process exit code. The optional third argument scripts the two UI-state
// buckets the app preloads (see `uiStateOptions`); every fetch the app
// makes, and every service resolution (method RESOLVE), lands in
// ctx.fetchLog in order, and ctx.resolveService(id) reads any registry
// service the boot bound. Run after `npm run build`.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { JSDOM } from "jsdom";
import { attachSeams, distDir } from "./bundle-seams.mjs";
import { assertNoLeaks } from "./leak-check.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// The UI-state routes the composition root preloads at boot and the stores
// write through (src/services/ui-storage.ts). GET answers the bucket's
// document; PUT /{key} answers `{ saved: true }`.
const UI_STATE_ROUTES = {
  "/user/state": "user",
  "/workspace/file/state": "workspace",
};

const jsonResponse = (body, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

/**
 * Normalizes `options.uiState` into one entry per bucket. Each bucket is
 * scripted as `"resolve"` (the default: an empty document, every key
 * absent), `"reject"` (the fetch rejects with a network error), `"hang"`
 * (the fetch never settles, so the app's preload timeout decides), or an
 * object (the fetch resolves with that object as the bucket's document, the
 * way a test seeds stores with values). Unknown modes fail loudly: a typo
 * would otherwise silently boot on defaults.
 */
function uiStateOptions(uiState = {}) {
  const buckets = {};
  for (const bucket of ["user", "workspace"]) {
    const mode = uiState[bucket] ?? "resolve";
    if (typeof mode === "object" && mode !== null) {
      buckets[bucket] = { mode: "resolve", body: mode };
    } else if (mode === "resolve") {
      buckets[bucket] = { mode, body: {} };
    } else if (mode === "reject" || mode === "hang") {
      buckets[bucket] = { mode, body: null };
    } else {
      throw new Error(`uiState.${bucket} must be resolve, reject, hang, or an object; got ${mode}`);
    }
  }
  return buckets;
}

/**
 * Boots the bundled workbench in jsdom and runs `run(ctx)` - the test body -
 * under the shared disposable-leak check: a body that leaves a
 * DisposableStore created during the run undisposed fails the test. The
 * app's own boot tree is exempt by construction (the tracker installs after
 * boot settles); it lives for the page lifetime by design.
 *
 * `ctx` holds the window, the scripted-socket registry, the status bar
 * elements, the fetch log, and the push helpers; `run` records failed
 * expectations by pushing plain-English messages onto ctx.failures.
 * `options.uiState` scripts the two UI-state buckets (see uiStateOptions).
 * This function never returns: it prints the verdict and exits the process,
 * because pending app timers (the status-bar LED pulse, reconnect backoffs)
 * outlive the assertions.
 */
export async function bootWorkbench(name, run, options = {}) {
  const uiState = uiStateOptions(options.uiState);
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
  const { window } = dom;

  // jsdom lacks layout APIs the panels touch; no-op stubs are enough because
  // nothing scrolls in the tests.
  window.matchMedia =
    window.matchMedia ||
    (() => ({
      matches: false,
      media: "",
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
      removeListener() {},
      dispatchEvent: () => false,
    }));
  window.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  window.IntersectionObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
    takeRecords() {
      return [];
    }
  };
  window.Element.prototype.scrollTo = () => {};
  window.HTMLElement.prototype.scrollIntoView = () => {};
  // jsdom's Range has no layout rects; ProseMirror's scroll-to-selection
  // reads them when a landed dictation final focuses the prompt editor.
  window.Range.prototype.getClientRects = () => [];
  window.Range.prototype.getBoundingClientRect = () => new window.DOMRect();

  // A scripted WebSocket stands in for the server's persistent sockets:
  // the workshop /ws connection the composition root opens, and the
  // /agents/ws connection the agent panel opens. It must live on
  // globalThis: the bundle calls the global `WebSocket`, not
  // `window.WebSocket`. Frames a test wants answered are pushed through
  // the socket's own onmessage by the ctx helpers below.
  const sockets = [];
  const realtimeSession = (include, prompt = "") => ({
    id: "boot_realtime",
    object: "realtime.transcription_session",
    type: "transcription",
    audio: {
      input: {
        format: { type: "audio/pcm", rate: 24000 },
        noise_reduction: null,
        transcription: { model: "realtime-transcribe", prompt },
        turn_detection: null,
      },
    },
    include,
  });
  class FakeWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    constructor(url) {
      this.url = url;
      this.readyState = FakeWebSocket.CONNECTING;
      this.sent = [];
      sockets.push(this);
      setTimeout(() => {
        this.readyState = FakeWebSocket.OPEN;
        this.onopen?.();
        if (this.url.endsWith("/v1/realtime")) {
          this.onmessage?.({
            data: JSON.stringify({
              type: "session.created",
              event_id: "boot_realtime_created",
              session: realtimeSession([]),
            }),
          });
        }
      }, 0);
    }
    addEventListener(type, listener) {
      const prop = `on${type}`;
      const previous = this[prop];
      this[prop] = previous ? (event) => (previous(event), listener(event)) : listener;
    }
    send(data) {
      this.sent.push(data);
      const event = typeof data === "string" ? JSON.parse(data) : null;
      if (event?.type === "session.update") {
        queueMicrotask(() =>
          this.onmessage?.({
            data: JSON.stringify({
              type: "session.updated",
              event_id: "boot_realtime_updated",
              session: realtimeSession(
                ["item.input_audio_transcription.hypothesis"],
                event.session.audio.input.transcription.prompt,
              ),
            }),
          }),
        );
      }
    }
    close() {
      this.readyState = FakeWebSocket.CLOSED;
    }
  }
  globalThis.WebSocket = FakeWebSocket;

  // Dictation stubs: jsdom has no audio stack, so the mic button's
  // getUserMedia/AudioContext path is scripted to succeed. The bundle
  // reads the globals.
  const fakeAudioStream = { getTracks: () => [{ stop() {} }] };
  globalThis.navigator.mediaDevices = {
    getUserMedia: () => Promise.resolve(fakeAudioStream),
  };
  class FakeAudioContext {
    constructor() {
      this.sampleRate = 24_000;
      this.destination = {};
      this.audioWorklet = { addModule: () => Promise.resolve() };
    }
    createMediaStreamSource() {
      return { connect() {}, disconnect() {} };
    }
    close() {
      return Promise.resolve();
    }
    resume() {
      return Promise.resolve();
    }
  }
  class FakeAudioWorkletNode {
    constructor() {
      this.port = {
        onmessage: null,
        postMessage: (message) => {
          if (message?.type === "flush") {
            queueMicrotask(() => this.port.onmessage?.({ data: { type: "flushed" } }));
          }
        },
      };
    }
    connect() {}
    disconnect() {}
  }
  window.AudioContext = FakeAudioContext;
  globalThis.AudioContext = FakeAudioContext;
  globalThis.AudioWorkletNode = FakeAudioWorkletNode;

  // The workbench state (models, profiles, selection) arrives only over
  // the socket, so a booted workbench fetches nothing but the two UI-state
  // buckets the composition root preloads (scripted per `options.uiState`)
  // and the Workshop tree's roots listing (answered empty: no grants yet).
  // Store writes PUT to the state routes and are answered saved. Any other
  // fetch, including the retired /v1/models and /profiles boot fetches,
  // rejects the test. Every call is recorded in order, with a timestamp
  // and the parsed PUT body, so a test can assert what the app fetched and
  // when relative to the rest of boot; the service observer installed
  // below interleaves getService resolutions into the same log.
  const fetchLog = [];
  const bootStart = Date.now();
  globalThis.fetch = (url, init = {}) => {
    const method = init.method ?? "GET";
    const entry = { url, method, at: Date.now() - bootStart };
    if (method === "PUT" && typeof init.body === "string") {
      entry.body = JSON.parse(init.body);
    }
    fetchLog.push(entry);
    if (url === "/workspace/tree") {
      return Promise.resolve(jsonResponse({ path: null, entries: [] }));
    }
    const bucket = UI_STATE_ROUTES[url];
    if (bucket !== undefined && method === "GET") {
      const script = uiState[bucket];
      if (script.mode === "hang") {
        return new Promise(() => {});
      }
      if (script.mode === "reject") {
        return Promise.reject(new TypeError(`scripted network failure for ${url}`));
      }
      return Promise.resolve(jsonResponse(script.body));
    }
    const putRoute = Object.keys(UI_STATE_ROUTES).find((route) => url.startsWith(`${route}/`));
    if (putRoute !== undefined && method === "PUT") {
      return Promise.resolve(jsonResponse({ saved: true }));
    }
    return Promise.reject(new Error(`unexpected fetch in a booted workbench test: ${url}`));
  };

  for (const key of [
    "document",
    "navigator",
    "location",
    "localStorage",
    "HTMLElement",
    "HTMLTemplateElement",
    "HTMLInputElement",
    "HTMLTextAreaElement",
    "HTMLButtonElement",
    "Node",
    "Element",
    "Event",
    "CustomEvent",
    "MutationObserver",
    "Option",
    "DOMParser",
    "NodeFilter",
    "ResizeObserver",
    "IntersectionObserver",
    "getComputedStyle",
    "requestAnimationFrame",
    "cancelAnimationFrame",
  ]) {
    if (!(key in globalThis) && key in window) {
      globalThis[key] = window[key];
    }
  }
  // Node ships its own Event and CustomEvent globals, so the copy loop skips
  // them - but events the bundle dispatches into the jsdom document must be
  // jsdom-realm instances: jsdom's dispatchEvent rejects Node's Event with
  // "parameter 1 is not of type 'Event'".
  globalThis.Event = window.Event;
  globalThis.CustomEvent = window.CustomEvent;
  globalThis.window = window;
  globalThis.document = window.document;

  // The bundle's test-only seams (the disposable tracker, the service
  // observer) are tree-shaken out of the entry; bundle-seams.mjs reattaches
  // them as appended exports and answers the chunk path holding each.
  const seams = await attachSeams();
  // The service observer must be live before the entry evaluates: the
  // resolutions it records happen during boot itself. Importing the seam's
  // chunk first evaluates only that shared chunk (the registry and its
  // base-layer dependencies), and the entry's later import reuses the
  // evaluated instance. Every getService call lands in fetchLog beside the
  // fetches, under the RESOLVE method, so a test reads one ordered log.
  const serviceSeam = await import(pathToFileURL(seams.__setServiceObserver).href);
  serviceSeam.__setServiceObserver({
    serviceResolved: (id) => {
      fetchLog.push({ url: id, method: "RESOLVE", at: Date.now() - bootStart });
    },
  });
  // The entry's name is content-hashed (dist/bundle/app-<hash>.js); the
  // build's manifest maps the logical name to it.
  const manifest = JSON.parse(await readFile(path.join(distDir, "manifest.json"), "utf8"));
  await import(pathToFileURL(path.join(distDir, manifest["app.js"])).href);
  const trackerSeam = await import(pathToFileURL(seams.__setDisposableTracker).href);
  const lifecycle = { setDisposableTracker: trackerSeam.__setDisposableTracker };

  // Resolves a registry service by token id the way the bundle's own
  // getService does (building it on first use from the current factory),
  // so a test can read a store the boot bound - including one no consumer
  // has resolved yet - without a seam per store. Throws on an unknown id.
  const registrySeam = await import(pathToFileURL(seams.__serviceRegistrations).href);
  function resolveService(id) {
    for (const [token, registration] of registrySeam.__serviceRegistrations()) {
      if (token.id !== id) continue;
      if (!registration.built) {
        registration.instance = registration.factory();
        registration.built = true;
      }
      return registration.instance;
    }
    throw new Error(`no service registered for ${id}`);
  }

  const statusBar = window.document.querySelector(".status-bar");
  const statusText = window.document.querySelector(".status-bar__text");
  const statusSlot = window.document.querySelector(".status-bar__slot");
  const barberpoleEl = window.document.querySelector(".status-bar__barberpole");
  const indicatorsEl = window.document.querySelector(".status-bar__indicators");
  const ledEl = window.document.querySelector(".status-bar__led:not(.status-bar__led--rec)");
  const recEl = window.document.querySelector(".status-bar__led--rec");

  // Every booted test reads the status bar and the mounted workbench; a
  // boot without them is broken, not a per-feature failure, so fail
  // loudly here. The agent panel mounts a beat after the dock, so poll.
  let agentPanel = null;
  for (let i = 0; i < 100 && !agentPanel; i++) {
    agentPanel = window.document.querySelector("#dock .ws-agent-panel");
    if (!agentPanel) await sleep(20);
  }
  const missing = [
    ["the status bar", statusBar],
    ["the ws-agent-session panel", agentPanel],
    ["the Workshop tree", window.document.querySelector("#dock .ws-workshop-tree")],
  ]
    .filter(([, node]) => !node)
    .map(([what]) => what);
  if (missing.length > 0) {
    throw new Error(`the workbench did not boot: ${missing.join(", ")} never mounted`);
  }

  // The composition root's own workshop socket: /ws exactly, never the
  // agent panel's /agents/ws connection.
  const wsSocket = () =>
    sockets.filter((socket) => socket.url.endsWith("/ws") && !socket.url.endsWith("/agents/ws")).at(-1);
  // The agent panel's session socket, and the per-take Realtime sockets
  // the mic opens.
  const agentsSocket = () => sockets.filter((socket) => socket.url.endsWith("/agents/ws")).at(-1);
  const sttSockets = () => sockets.filter((socket) => socket.url.endsWith("/v1/realtime"));

  // The fake socket flips to OPEN on a 0ms timer, and the app can boot
  // during the bundle import's own microtask drain - before any macrotask
  // ran. Wait for the boot socket to open so no test body observes (or
  // drops) a socket that is still CONNECTING: a real WebSocket never fires
  // open after close, but the fake's late timer would, and that stale
  // onopen cancels the reconnect backoff the close just scheduled.
  const socketDeadline = Date.now() + 5000;
  while (wsSocket()?.readyState !== FakeWebSocket.OPEN && Date.now() < socketDeadline) {
    await sleep(10);
  }
  if (wsSocket()?.readyState !== FakeWebSocket.OPEN) {
    throw new Error("the workbench did not boot: the /ws socket never opened");
  }

  // Pushes one observer status frame down the persistent socket, as the
  // server's /ws route would. Fields default to a plain idle update.
  function emitStatus(overrides = {}) {
    wsSocket()?.onmessage?.({
      data: JSON.stringify({
        type: "status",
        label: "Ready",
        description: "",
        severity: "info",
        activity: "general",
        busy: false,
        ...overrides,
      }),
    });
  }

  // Pushes a models frame, as the server does when the gateway returns
  // after an outage.
  function emitModels(models) {
    wsSocket()?.onmessage?.({ data: JSON.stringify({ type: "models", models }) });
  }

  // Pushes one complete workbench snapshot, as the server's /ws route
  // does whenever its menu state changes. Fields default to the booted
  // single-model state.
  function emitWorkbench(overrides = {}) {
    wsSocket()?.onmessage?.({
      data: JSON.stringify({
        type: "workbench",
        profiles: [],
        active: null,
        switching: null,
        switch_in_flight: false,
        selected: "test-model",
        chat_ready: true,
        ...overrides,
      }),
    });
  }

  // Pushes one frame down the agent panel's /agents/ws socket, as the
  // server's agent route would: a session acknowledgment, an event, a
  // wait announcement.
  function emitAgent(frame) {
    agentsSocket()?.onmessage?.({ data: JSON.stringify(frame) });
  }

  // The server pushes the retained status, the model catalog, and a
  // workbench snapshot on connect, in that order (session.rs) - the app
  // makes no HTTP state fetches at boot. Mirror all three pushes here:
  // the status seeds the status bar, the catalog populates the agent
  // toolbar's model picker, and the snapshot names the selection.
  emitStatus();
  emitModels([{ id: "test-model", description: "scripted" }]);
  emitWorkbench();

  const failures = [];

  const ctx = {
    window,
    document: window.document,
    sockets,
    FakeWebSocket,
    wsSocket,
    agentsSocket,
    sttSockets,
    emitAgent,
    agentPanel,
    statusBar,
    statusText,
    statusSlot,
    barberpoleEl,
    indicatorsEl,
    ledEl,
    recEl,
    emitStatus,
    emitModels,
    emitWorkbench,
    fetchLog,
    resolveService,
    sleep,
    failures,
  };

  try {
    await assertNoLeaks(lifecycle, () => run(ctx));
  } catch (error) {
    // Either the leak report or a crash in the test body; both fail the
    // test with the message in the verdict.
    failures.push(error?.stack ?? String(error));
  }

  if (failures.length > 0) {
    console.error(`${name} failed:\n- ${failures.join("\n- ")}`);
    process.exit(1);
  }
  console.log(`${name} passed`);
  process.exit(0);
}
