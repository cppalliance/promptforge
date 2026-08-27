// Shared browser-environment fixture for the bundle-level workshop tests:
// the jsdom boot that smoke.mjs originally built inline, extracted so every
// per-feature slice boots the exact same way. bootWorkbench(name, run)
// loads dist/index.html into jsdom, stands in fakes for the APIs jsdom
// lacks (WebSocket, audio capture, fetch, layout metrics), imports the
// bundled dist/app.js, waits for the app to settle, then runs `run` under
// the shared disposable-leak check and reports the verdict through the
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
 * `ctx` carries the window, the scripted-socket registry, the composer
 * elements, and the interaction helpers; `run` records failed expectations
 * by pushing plain-English messages onto ctx.failures.
 * This function never returns: it prints the verdict and exits the process,
 * because pending app timers (the voice stop grace window, the status-bar
 * LED pulse) outlive the assertions.
 */
export async function bootWorkbench(name, run) {
  const html = await readFile(path.join(distDir, "index.html"), "utf8");
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
  const { window } = dom;

  // jsdom lacks layout APIs the feed touches; no-op stubs are enough because
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
  // jsdom has no layout engine, so scrollHeight is always 0 and murm-ui's
  // adjustHeight would pin the composer at 0px. Simulate line-based metrics
  // for textareas so composer auto-growth is observable as inline height.
  Object.defineProperty(window.HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get() {
      if (this instanceof window.HTMLTextAreaElement) {
        return 36 + (this.value.split("\n").length - 1) * 21;
      }
      return 0;
    },
  });

  // A scripted WebSocket stands in for the server's persistent /ws route. It
  // must live on globalThis: the bundle calls the global `WebSocket`, not
  // `window.WebSocket`. The app opens one socket on load; each chat frame
  // sent on it is captured and answered with two delta frames and a done
  // frame echoing the frame's id, scheduled in order so the provider's
  // round-trip runs. The socket stays open after `done` - it is persistent.
  const chatSockets = [];
  class FakeWebSocket {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    constructor(url) {
      this.url = url;
      this.readyState = FakeWebSocket.CONNECTING;
      // Mid-stream hang mode: answer a chat frame with one delta and no
      // done, so the generation stays in flight until the client aborts.
      this.hangChat = false;
      // Reasoning mode: stream two reasoning frames before the content
      // deltas, as a reasoning model's side channel would.
      this.reasonChat = false;
      chatSockets.push(this);
      setTimeout(() => {
        this.readyState = FakeWebSocket.OPEN;
        this.onopen?.();
      }, 0);
    }
    // The voice path attaches with addEventListener; chain listeners onto the
    // on* properties the chat path assigns directly.
    addEventListener(type, listener) {
      const prop = `on${type}`;
      const previous = this[prop];
      this[prop] = previous ? (event) => (previous(event), listener(event)) : listener;
    }
    send(data) {
      let frame;
      try {
        frame = JSON.parse(data);
      } catch {
        return; // voice control words ("start"/"stop") are not JSON
      }
      if (frame.type !== "chat") return;
      this.chatFrame = frame;
      if (this.hangChat) {
        queueMicrotask(() =>
          this.onmessage?.({ data: JSON.stringify({ type: "delta", content: "partial", id: frame.id }) }),
        );
        return;
      }
      const frames = [];
      if (this.reasonChat) {
        frames.push(
          { type: "reasoning", content: "consider the ask", id: frame.id },
          { type: "reasoning", content: " then answer", id: frame.id },
        );
      }
      frames.push(
        { type: "delta", content: "Hello", id: frame.id },
        { type: "delta", content: " back [docs](https://example.com/)", id: frame.id },
        { type: "done", id: frame.id },
      );
      for (const reply of frames) {
        queueMicrotask(() => this.onmessage?.({ data: JSON.stringify(reply) }));
      }
    }
    close() {
      this.readyState = FakeWebSocket.CLOSED;
    }
  }
  globalThis.WebSocket = FakeWebSocket;

  // Voice capture stubs: jsdom has no audio stack, so the mic button's
  // getUserMedia/AudioContext path is scripted to succeed. The bundle reads
  // the globals, so they land on both window and globalThis; `navigator` is
  // Node's own global (the key-copy loop below skips keys already present),
  // so mediaDevices goes on it directly.
  const fakeAudioStream = { getTracks: () => [{ stop() {} }] };
  const fakeMediaDevices = { getUserMedia: () => Promise.resolve(fakeAudioStream) };
  window.navigator.mediaDevices = fakeMediaDevices;
  globalThis.navigator.mediaDevices = fakeMediaDevices;
  class FakeAudioContext {
    constructor() {
      this.destination = {};
      this.audioWorklet = { addModule: () => Promise.resolve() };
    }
    createMediaStreamSource() {
      return { connect() {}, disconnect() {} };
    }
    close() {
      return Promise.resolve();
    }
  }
  class FakeAudioWorkletNode {
    constructor() {
      this.port = { onmessage: null };
    }
    connect() {}
    disconnect() {}
  }
  window.AudioContext = FakeAudioContext;
  globalThis.AudioContext = FakeAudioContext;
  window.AudioWorkletNode = FakeAudioWorkletNode;
  globalThis.AudioWorkletNode = FakeAudioWorkletNode;

  // A scripted fetch stands in for the model catalog. The catalog answers
  // with one model so it auto-selects and submission is unblocked; the
  // Workshop tree's boot fetch answers with an empty roots listing (no
  // grants yet); any other fetch - including the retired POST /chat SSE
  // path - rejects the test.
  globalThis.fetch = (url) => {
    if (url === "/v1/models") {
      return Promise.resolve(
        new Response(JSON.stringify({ data: [{ id: "test-model", description: "scripted" }] }), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      );
    }
    if (url === "/workspace/tree") {
      return Promise.resolve(
        new Response(JSON.stringify({ path: null, entries: [] }), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      );
    }
    if (url === "/voice/capability") {
      return Promise.resolve(
        new Response(JSON.stringify({ gpu: true }), {
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

  // The mic button waits on the GPU capability fetch, so it mounts a
  // microtask or two after the rest of the composer.
  let mic = null;
  for (let i = 0; i < 100 && !mic; i++) {
    mic = window.document.querySelector(".voice-mic");
    if (!mic) await sleep(20);
  }

  const input = window.document.querySelector(".mur-chat-input");
  const form = window.document.querySelector(".mur-chat-form");
  const history = window.document.querySelector(".mur-chat-history");
  const send = window.document.querySelector(".mur-send-btn");
  const statusBar = window.document.querySelector(".status-bar");
  const statusText = window.document.querySelector(".status-bar__text");
  const statusSlot = window.document.querySelector(".status-bar__slot");
  const progressEl = window.document.querySelector(".status-bar__progress");
  const ledEl = window.document.querySelector(".status-bar__led");
  const recEl = window.document.querySelector(".status-bar__rec");

  // Every interaction helper needs these four; a workbench without them is
  // a broken boot, not a per-feature failure, so fail loudly here.
  const missing = [
    ["the mic button", mic],
    ["the chat input", input],
    ["the chat form", form],
    ["the chat history", history],
  ]
    .filter(([, node]) => !node)
    .map(([what]) => what);
  if (missing.length > 0) {
    throw new Error(`the workbench did not boot: ${missing.join(", ")} never mounted`);
  }

  const wsSocket = () => chatSockets.filter((socket) => socket.url.endsWith("/ws")).at(-1);

  // The fake socket flips to OPEN on a 0ms timer, and the mic can mount
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

  // Drives one chat submission through murm-ui's real form handling and
  // waits for the scripted reply to render. Returns the chat frame the
  // provider sent (undefined when submission stayed blocked).
  async function submitChat(text) {
    const socket = wsSocket();
    const repliesBefore = (history.textContent.match(/Hello back/g) || []).length;
    input.value = text;
    input.dispatchEvent(new window.Event("input", { bubbles: true }));
    form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
    const replyDeadline = Date.now() + 5000;
    while (
      (history.textContent.match(/Hello back/g) || []).length === repliesBefore &&
      Date.now() < replyDeadline
    ) {
      await sleep(20);
    }
    return socket?.chatFrame;
  }

  // Clicks the mic and waits for the take's /voice socket to open with its
  // message listener wired. Returns the socket, or null when no take
  // started before the deadline.
  async function startTake() {
    const before = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
    mic.click();
    const openDeadline = Date.now() + 5000;
    while (Date.now() < openDeadline) {
      const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
      if (voiceSockets.length > before && typeof voiceSockets.at(-1).onmessage === "function") {
        return voiceSockets.at(-1);
      }
      await sleep(20);
    }
    return null;
  }

  const failures = [];

  const ctx = {
    window,
    document: window.document,
    chatSockets,
    FakeWebSocket,
    wsSocket,
    mic,
    input,
    form,
    history,
    send,
    statusBar,
    statusText,
    statusSlot,
    progressEl,
    ledEl,
    recEl,
    emitStatus,
    emitModels,
    submitChat,
    startTake,
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
