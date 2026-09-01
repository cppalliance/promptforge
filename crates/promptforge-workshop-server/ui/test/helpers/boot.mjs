// Shared browser-environment fixture for the bundle-level workshop tests:
// the jsdom boot that smoke.mjs originally built inline, extracted so every
// per-feature slice boots the exact same way. bootWorkbench(name, run)
// loads dist/index.html into jsdom, stands in fakes for the APIs jsdom
// lacks (WebSocket, fetch, layout metrics), imports the bundled
// dist/app.js, waits for the app to settle, then runs `run` under the
// shared disposable-leak check and reports the verdict through the
// process exit code. Run after `npm run build`.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./leak-check.mjs";

const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "dist");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Boots the bundled workbench in jsdom and runs `run(ctx)` - the test body -
 * under the shared disposable-leak check: a body that leaves a
 * DisposableStore created during the run undisposed fails the test. The
 * app's own boot tree is exempt by construction (the tracker installs after
 * boot settles); it lives for the page lifetime by design.
 *
 * `ctx` carries the window, the scripted-socket registry, the status bar
 * elements, and the push helpers; `run` records failed expectations by
 * pushing plain-English messages onto ctx.failures.
 * This function never returns: it prints the verdict and exits the process,
 * because pending app timers (the status-bar LED pulse, reconnect backoffs)
 * outlive the assertions.
 */
export async function bootWorkbench(name, run) {
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

  // A scripted WebSocket stands in for the server's persistent sockets:
  // the workshop /ws connection the composition root opens, and the
  // /agents/ws connection the agent panel opens. It must live on
  // globalThis: the bundle calls the global `WebSocket`, not
  // `window.WebSocket`. Frames a test wants answered are pushed through
  // the socket's own onmessage by the ctx helpers below.
  const sockets = [];
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
      }, 0);
    }
    addEventListener(type, listener) {
      const prop = `on${type}`;
      const previous = this[prop];
      this[prop] = previous ? (event) => (previous(event), listener(event)) : listener;
    }
    send(data) {
      this.sent.push(data);
    }
    close() {
      this.readyState = FakeWebSocket.CLOSED;
    }
  }
  globalThis.WebSocket = FakeWebSocket;

  // The workbench state (models, profiles, selection) arrives only over
  // the socket, so a booted workbench fetches nothing but the Workshop
  // tree's roots listing (answered empty: no grants yet) and the Gateway
  // Config panel's origin probe when that panel opens. Any other fetch -
  // including the retired /v1/models and /profiles boot fetches - rejects
  // the test.
  globalThis.fetch = (url) => {
    if (url === "/workspace/tree") {
      return Promise.resolve(
        new Response(JSON.stringify({ path: null, entries: [] }), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      );
    }
    if (url === "/gateway/origin") {
      return Promise.resolve(
        new Response(JSON.stringify({ origin: "http://127.0.0.1:8081" }), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      );
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

  // dist/app.js exports nothing (main.ts is an entry point) and esbuild
  // tree-shakes the unused setDisposableTracker export away, so the leak
  // check's seam is unreachable from outside the bundle. Reattach it by
  // appending one export to the bundle text and importing the result as a
  // data URL: the dist bytes execute unmodified, and the appended function
  // assigns the bundle's own module-scope tracker variable, located by its
  // single call site in the DisposableStore constructor.
  const bundleSource = await readFile(path.join(distDir, "app.js"), "utf8");
  const trackerVar = bundleSource.match(/(\w+)\?\.trackCreated\(this\)/)?.[1];
  if (!trackerVar) {
    throw new Error(
      "boot.mjs could not locate the disposable tracker seam in dist/app.js; rebuild dist or retune the seam regex",
    );
  }
  const patched = `${bundleSource}\nexport function __setDisposableTracker(next) { ${trackerVar} = next; }\n`;
  const bundle = await import(
    `data:text/javascript;base64,${Buffer.from(patched, "utf8").toString("base64")}`
  );
  const lifecycle = { setDisposableTracker: bundle.__setDisposableTracker };

  const statusBar = window.document.querySelector(".status-bar");
  const statusText = window.document.querySelector(".status-bar__text");
  const statusSlot = window.document.querySelector(".status-bar__slot");
  const progressEl = window.document.querySelector(".status-bar__progress");
  const indicatorsEl = window.document.querySelector(".status-bar__indicators");
  const ledEl = window.document.querySelector(".status-bar__led");
  const recEl = window.document.querySelector(".status-bar__rec");

  // Every booted test reads the status bar and the mounted workbench; a
  // boot without them is broken, not a per-feature failure, so fail
  // loudly here. The agent panel mounts a beat after the dock, so poll.
  let agentPanel = null;
  for (let i = 0; i < 100 && !agentPanel; i++) {
    agentPanel = window.document.querySelector("#dock .agent-panel");
    if (!agentPanel) await sleep(20);
  }
  const missing = [
    ["the status bar", statusBar],
    ["the agent-session panel", agentPanel],
    ["the Workshop tree", window.document.querySelector("#dock .workshop-tree")],
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
        progress: null,
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
        selected: "test-model",
        chat_ready: true,
        ...overrides,
      }),
    });
  }

  // The server pushes the retained status, the model catalog, and a
  // workbench snapshot on connect, in that order (session.rs) - the app
  // makes no HTTP state fetches at boot. Mirror all three pushes here:
  // the status seeds the status bar, the catalog populates the Model
  // menu, and the snapshot carries the selection.
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
    agentPanel,
    statusBar,
    statusText,
    statusSlot,
    progressEl,
    indicatorsEl,
    ledEl,
    recEl,
    emitStatus,
    emitModels,
    emitWorkbench,
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
