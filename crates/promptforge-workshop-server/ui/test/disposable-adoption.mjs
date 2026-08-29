// Integration test for the Disposable adoption (step 4): representative
// workshop components register their listeners, subscriptions, and timers
// through the lifecycle primitives, so one root dispose() severs them all.
// Bundles the TS modules with esbuild and drives them against jsdom built
// from the real index.html. Covers: setupWindowMenus releasing its
// document-level listeners and popovers, EditorPanel disposing its surface
// child and dirty subscription, StatusBar cancelling its LED decay timer,
// and PermanentTab dropping its title subscription (emitter delivery
// stops after the root disposes). A final section drives WorkshopSocket
// against a scripted fake WebSocket: emitter fan-out and unsubscribe, a
// normal server close still reconnecting, and disposal settling pending
// chats without a disconnect fan-out or reconnect.
// Run: node test/disposable-adoption.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { DisposableStore } from "./src/base/lifecycle.ts";
      export { Emitter } from "./src/base/event.ts";
      export { ModelService } from "./src/services/model-service.ts";
      export { WorkshopSocket } from "./src/services/workshop-socket.ts";
      export { StatusBar } from "./src/ui/status-bar.ts";
      export { setupWindowMenus } from "./src/ui/window-menu.ts";
      export { EditorPanel } from "./src/ui/workshop/editor-panel.ts";
      export { createPanelTabComponent, PERMANENT_TAB } from "./src/ui/workshop/panel-types.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.Element = window.Element;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLInputElement = window.HTMLInputElement;
globalThis.HTMLTextAreaElement = window.HTMLTextAreaElement;
globalThis.Node = window.Node;
globalThis.getComputedStyle = window.getComputedStyle.bind(window);

const bundlePath = path.join(os.tmpdir(), "promptforge-disposable-adoption-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  DisposableStore,
  Emitter,
  ModelService,
  WorkshopSocket,
  StatusBar,
  setupWindowMenus,
  EditorPanel,
  createPanelTabComponent,
  PERMANENT_TAB,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// The root of the tree under test: every component registers here and one
// dispose() at the end must sever everything.
const root = new DisposableStore();

// --- Spy on document-level listeners ---------------------------------------
// Every addEventListener on the document is tracked until its matching
// removeEventListener; after the root disposes, the live count must be
// back at the pre-setup baseline.

const liveDocListeners = new Set();
const realAddEventListener = window.document.addEventListener.bind(window.document);
const realRemoveEventListener = window.document.removeEventListener.bind(window.document);
window.document.addEventListener = (type, listener, options) => {
  liveDocListeners.add(listener);
  realAddEventListener(type, listener, options);
};
window.document.removeEventListener = (type, listener, options) => {
  liveDocListeners.delete(listener);
  realRemoveEventListener(type, listener, options);
};

// --- setupWindowMenus: document listeners and popovers ----------------------

const baselineListeners = liveDocListeners.size;
const menus = root.add(
  setupWindowMenus({
    agents: { newAgent: () => {} },
    workshop: { toggleWorkshopPanel: () => {} },
    modelMenu: new ModelService(() => true),
  }),
);
check("menu setup registers document-level listeners", liveDocListeners.size > baselineListeners);
check(
  "menu setup still returns the shared command set",
  typeof menus.newAgent === "function" && typeof menus.showAbout === "function",
);
const fileButton = window.document.querySelector('[data-menu="file"]');
fileButton.click();
const filePopover = fileButton.nextElementSibling;
check("the File menu opens before disposal", filePopover !== null && filePopover.hidden === false);
fileButton.click();
check("the File menu closes again before disposal", filePopover.hidden === true);

// --- EditorPanel: surface child and dirty subscription ----------------------

function createStubSurface() {
  const listeners = new Set();
  return {
    element: window.document.createElement("div"),
    disposeCount: 0,
    dirty: false,
    currentText: "",
    open(document) {
      this.currentText = document.text;
      this.setDirty(false);
    },
    text() {
      return this.currentText;
    },
    markSaved(text) {
      this.setDirty(this.currentText !== text);
    },
    isDirty() {
      return this.dirty;
    },
    onDirtyChange(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    focus() {},
    dispose() {
      this.disposeCount += 1;
    },
    setDirty(dirty) {
      if (dirty === this.dirty) return;
      this.dirty = dirty;
      for (const listener of listeners) listener(dirty);
    },
  };
}

const stub = createStubSurface();
const titles = [];
const panel = root.add(
  new EditorPanel({
    createSurface: () => stub,
    readFile: async () => ({ path: "C:\\ws\\a.txt", size: 6, token: "t1", text: "hello\n" }),
    writeFile: async () => {
      throw new Error("no writes in this test");
    },
  }),
);
panel.init({ params: { path: "C:\\ws\\a.txt" }, api: { setTitle: (title) => titles.push(title) } });
await flush();
check("the panel loads its document before disposal", stub.currentText === "hello\n");
stub.setDirty(true);
check("the dirty subscription delivers before disposal", titles.at(-1) === "● a.txt");
stub.setDirty(false);

// --- StatusBar: the LED decay timer -----------------------------------------

const statusBar = root.add(new StatusBar(window.document.querySelector(".status-bar")));
const led = window.document.querySelector(".status-bar__led");
const generatingFrame = {
  type: "status",
  label: "Working",
  description: "",
  severity: "info",
  activity: "generating",
  progress: null,
};
statusBar.render(generatingFrame);
check(
  "a generating frame lights the LED",
  led.classList.contains("status-bar__led--generating"),
);
// Control: while the bar is live, the decay timer clears the pulse.
await sleep(400);
check(
  "the live decay timer clears the pulse",
  !led.classList.contains("status-bar__led--generating"),
);
// Re-arm the pulse; disposal must cancel this timer.
statusBar.render(generatingFrame);
check(
  "the re-armed pulse lights the LED again",
  led.classList.contains("status-bar__led--generating"),
);

// --- PermanentTab: the title subscription ------------------------------------

const tab = root.add(createPanelTabComponent({ name: PERMANENT_TAB }));
const titleChanges = new Emitter();
tab.init({ title: "Workshop", api: { onDidTitleChange: titleChanges.event }, tabLocation: "header" });
check("the permanent tab renders its initial title", tab.element.textContent === "Workshop");
titleChanges.fire({ title: "Renamed" });
check("the tab follows title changes before disposal", tab.element.textContent === "Renamed");

// --- One dispose() up the tree ------------------------------------------------

root.dispose();

// Window menus: document listeners gone, popovers (and their row
// listeners) out of the DOM, buttons inert.
check(
  "disposal releases every document-level listener the menus added",
  liveDocListeners.size === baselineListeners,
);
check(
  "disposal removes the menu popovers",
  window.document.querySelectorAll(".window-titlebar__popover").length === 0,
);
fileButton.click();
check(
  "a disposed menu button no longer opens anything",
  window.document.querySelectorAll(".window-titlebar__popover").length === 0,
);

// EditorPanel: the surface child disposed once, the dirty subscription severed.
check("disposal reaches the panel's surface child exactly once", stub.disposeCount === 1);
const titlesBefore = titles.length;
stub.setDirty(true);
check("the dirty subscription is severed after disposal", titles.length === titlesBefore);

// StatusBar: the armed decay timer was cancelled, so the pulse never decays.
await sleep(400);
check(
  "disposal cancels the LED decay timer",
  led.classList.contains("status-bar__led--generating"),
);

// PermanentTab: emitter delivery stops.
titleChanges.fire({ title: "After" });
check("the disposed tab ignores title changes", tab.element.textContent === "Renamed");

// A second root dispose is harmless.
root.dispose();

// --- WorkshopSocket: emitter fan-out, reconnect, disposal -------------------
// A scripted fake WebSocket drives the socket through open, a
// server-initiated close, the reconnect that must follow it, and disposal,
// which must settle pending chats without a disconnect fan-out or reconnect.

const fakeSockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  closed = false;
  sent = [];
  onopen = null;
  onclose = null;
  onerror = null;
  onmessage = null;
  constructor(url) {
    this.url = url;
    fakeSockets.push(this);
  }
  send(data) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
    this.readyState = 3;
  }
  // Test-side controls, not part of the WebSocket surface.
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  serverClose() {
    this.readyState = 3;
    this.onclose?.();
  }
  message(frame) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}
globalThis.WebSocket = FakeWebSocket;

const socket = new WorkshopSocket("ws://fake/ws");
const statusOrder = [];
const firstSub = socket.onStatus(() => statusOrder.push("first"));
socket.onStatus(() => statusOrder.push("second"));
let disconnects = 0;
socket.onDisconnect(() => (disconnects += 1));
// Handlers wired: declare readiness so pushes deliver instead of queueing
// (the boot queue has its own test, boot-queue.mjs).
socket.ready();

socket.connect();
check("connect opens one underlying socket", fakeSockets.length === 1);
fakeSockets[0].open();
const statusFrame = {
  type: "status",
  label: "Working",
  description: "",
  severity: "info",
  activity: null,
  progress: null,
};
fakeSockets[0].message(statusFrame);
check(
  "both status subscribers receive the frame in subscription order",
  statusOrder.join(",") === "first,second",
);
firstSub.dispose();
fakeSockets[0].message(statusFrame);
check(
  "a disposed subscription no longer receives frames",
  statusOrder.join(",") === "first,second,second",
);

// A chat that already streamed content resolves when the server drops the
// socket under it.
let chat1Settled = false;
const chat1 = socket
  .streamChat({ messages: [] }, { onDelta: () => {} }, new AbortController().signal)
  .then(() => {
    chat1Settled = true;
  });
await flush();
fakeSockets[0].message({ type: "delta", id: 1, content: "partial" });

// A server-initiated close is a dropout: the disconnect fan-out fires and
// the backoff opens a fresh socket - disposal must not have changed this.
fakeSockets[0].serverClose();
await chat1;
check("a started chat resolves when the server closes under it", chat1Settled === true);
check("a server-initiated close fires onDisconnect", disconnects === 1);
await sleep(1200); // one full RECONNECT_INITIAL_MS backoff step
check("a server-initiated close schedules a reconnect", fakeSockets.length === 2);

// Disposal is not a dropout: pending chats settle, but there is no
// disconnect fan-out and no reconnect.
fakeSockets[1].open();
let chat2Error = null;
const chat2 = socket
  .streamChat({ messages: [] }, { onDelta: () => {} }, new AbortController().signal)
  .catch((error) => {
    chat2Error = error;
  });
await flush();
socket.dispose();
await chat2;
check(
  "disposal rejects a not-yet-started chat instead of hanging it",
  chat2Error instanceof Error,
);
check("disposal closes the underlying socket", fakeSockets[1].closed === true);
check("disposal detaches onclose before closing", fakeSockets[1].onclose === null);
check("disposal does not fire onDisconnect", disconnects === 1);
const deliveredBeforeDisposal = statusOrder.length;
fakeSockets[1].message(statusFrame);
check("no status handler fires after disposal", statusOrder.length === deliveredBeforeDisposal);
await sleep(1200);
check("no reconnect happens after disposal", fakeSockets.length === 2);

if (failures.length > 0) {
  console.error(`disposable-adoption: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("disposable-adoption: all assertions passed");
process.exit(0);
