// Smoke test: loads dist/index.html into jsdom, imports the bundled
// dist/app.js, asserts the chat UI mounts without throwing, and drives one
// chat round-trip through a scripted WebSocket. Guards the DOM contract
// between index.html and the vendored murm-ui (its components throw when a
// required class is missing) and the wire contract of WorkbenchProvider
// (chat frame shape against /ws, delta frames rendered into the history).
// Run after `npm run build`: `npm test`.
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "..", "dist");

const html = await readFile(path.join(distDir, "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });

const { window } = dom;

// jsdom lacks layout APIs the feed touches; no-op stubs are enough because
// nothing scrolls in the test.
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
    chatSockets.push(this);
    queueMicrotask(() => {
      this.readyState = FakeWebSocket.OPEN;
      this.onopen?.();
    });
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
    const frames = [
      { type: "delta", content: "Hello", id: frame.id },
      { type: "delta", content: " back", id: frame.id },
      { type: "done", id: frame.id },
    ];
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

// Pushes one observer status frame down the persistent socket, as the
// server's /ws route would. Fields default to a plain idle update.
function emitStatus(overrides = {}) {
  const socket = chatSockets[0];
  socket?.onmessage?.({
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
// A scripted fetch stands in for the model catalog. The catalog answers with
// one model so the picker enables and submission is unblocked; any other
// fetch - including the retired POST /chat SSE path - rejects the test.
globalThis.fetch = (url) => {
  if (url === "/v1/models") {
    return Promise.resolve(
      new Response(JSON.stringify({ data: [{ id: "test-model", description: "scripted" }] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
  }
  return Promise.reject(new Error(`unexpected fetch in the smoke test: ${url}`));
};

for (const key of [
  "document",
  "navigator",
  "location",
  "localStorage",
  "HTMLElement",
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
globalThis.window = window;
globalThis.document = window.document;

await import(pathToFileURL(path.join(distDir, "app.js")).href);

// The bundle mounts dockview on #dock with one chat panel, and ChatUI on
// the .mur-app inside it: a successful mount leaves the murm structure
// intact and renders the empty-chat state.
const dock = window.document.querySelector("#dock");
const app = window.document.querySelector("#dock .mur-app");
const history = window.document.querySelector(".mur-chat-history");
const input = window.document.querySelector(".mur-chat-input");
const send = window.document.querySelector(".mur-send-btn");
const mic = window.document.querySelector(".voice-mic");
const statusBar = window.document.querySelector(".status-bar");
const statusText = window.document.querySelector(".status-bar__text");
const statusSlot = window.document.querySelector(".status-bar__slot");
const progressEl = window.document.querySelector(".status-bar__progress");
const ledEl = window.document.querySelector(".status-bar__led");

const failures = [];
if (!dock) failures.push("#dock missing");
if (dock && !dock.querySelector(".dv-dockview")) {
  failures.push("dockview did not initialize inside #dock");
}
if (!window.document.querySelector("#dock .dv-groupview")) {
  failures.push("dockview rendered no group for the chat panel");
}
if (!app) failures.push(".mur-app missing inside the dock");
if (!history) failures.push(".mur-chat-history missing");
if (!input) failures.push(".mur-chat-input missing");
if (!send) failures.push(".mur-send-btn missing");
if (!mic) failures.push("voice plugin did not insert the mic button");
if (!statusBar) failures.push("status bar placeholder missing");
if (statusBar && statusBar.tagName !== "FOOTER") {
  failures.push("the status bar is not a <footer> landmark");
}
if (!statusText) {
  failures.push("status bar text element missing");
} else if (statusText.textContent !== "Ready") {
  failures.push(`status bar placeholder text is "${statusText.textContent}", expected "Ready"`);
}
if (!statusSlot) failures.push("status bar slot missing");
if (!progressEl) {
  failures.push("status bar progress element missing");
} else if (!progressEl.hidden) {
  failures.push("progress bar must start hidden");
}
if (!ledEl) failures.push("status bar activity LED missing");
if (app && !app.classList.contains("mur-chat-empty")) {
  failures.push("fresh mount is not in the empty-chat state");
}

// One chat round-trip: wait for the scripted catalog to enable the picker,
// submit a message through the form, and assert the provider's chat frame
// shape and the rendered assistant reply.
const picker = window.document.getElementById("model-picker");
const form = window.document.querySelector(".mur-chat-form");
const deadline = Date.now() + 5000;
while (picker && picker.disabled && Date.now() < deadline) {
  await new Promise((resolve) => setTimeout(resolve, 20));
}
if (!picker || picker.disabled) {
  failures.push("scripted model catalog did not enable the picker");
} else if (input && form && history) {
  input.value = "Hello?";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
  const replyDeadline = Date.now() + 5000;
  while (!history.textContent.includes("Hello back") && Date.now() < replyDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!history.textContent.includes("Hello back")) {
    failures.push("assistant reply did not render in the chat history");
  }
  const socket = chatSockets[0];
  if (!socket) {
    failures.push("no /ws socket was opened");
  } else {
    if (chatSockets.length !== 1) {
      failures.push(`expected one persistent /ws socket, saw ${chatSockets.length}`);
    }
    if (!socket.url.endsWith("/ws")) failures.push(`chat socket opened the wrong URL: ${socket.url}`);
    const request = socket.chatFrame;
    if (!request) {
      failures.push("no chat frame was sent on the socket");
    } else {
      if (request.type !== "chat") failures.push("the frame is not a chat frame");
      if (typeof request.id !== "number") failures.push("chat frame carried no numeric id");
      if (request.model !== "test-model") failures.push("chat frame carried the wrong model");
      if ("stream" in request) failures.push("chat frame must not carry a stream flag");
      const first = request.messages?.[0];
      if (!first || first.role !== "user" || first.content !== "Hello?") {
        failures.push("chat frame messages are not the OpenAI shape");
      }
    }
  }
}

// Models frames: a pushed catalog (sent when the gateway comes back after
// an outage) refreshes the picker without a fetch. A selection that
// survives the new catalog is kept; one that vanished falls back to the
// first entry.
function emitModels(models) {
  const socket = chatSockets[0];
  socket?.onmessage?.({ data: JSON.stringify({ type: "models", models }) });
}
if (picker && !picker.disabled) {
  emitModels([
    { id: "test-model", description: "scripted" },
    { id: "fresh-model", description: "pushed" },
  ]);
  const ids = [...picker.options].map((option) => option.value);
  if (ids.join(",") !== "test-model,fresh-model") {
    failures.push(`a models frame did not rebuild the picker: ${ids.join(",")}`);
  }
  if (picker.value !== "test-model") {
    failures.push(`a surviving selection was not kept across the refresh: ${picker.value}`);
  }
  emitModels([{ id: "fresh-model", description: "pushed" }]);
  if (picker.value !== "fresh-model") {
    failures.push(`a vanished selection did not fall back to the first entry: ${picker.value}`);
  }
}

// Status frames: the observer's updates render into the status bar. Info
// and error frames set the text and tooltip; debug frames are internal
// instrumentation and must not touch either.
if (statusText && statusBar) {
  emitStatus({
    label: "Streaming response...",
    description: "gateway stream open",
    activity: "thinking",
  });
  if (statusText.textContent !== "Streaming response...") {
    failures.push("a status frame did not update the bar text");
  }
  if (statusBar.title !== "gateway stream open") {
    failures.push("the status description did not land on the bar tooltip");
  }
  emitStatus({ label: "per-delta pulse", description: "debug", severity: "debug", activity: "generating" });
  if (statusText.textContent !== "Streaming response...") {
    failures.push("a debug status frame changed the bar text");
  }
  if (statusBar.title !== "gateway stream open") {
    failures.push("a debug status frame changed the tooltip");
  }
  emitStatus({
    label: "Gateway error: 500",
    description: "upstream declined",
    severity: "error",
    activity: "general",
  });
  if (statusText.textContent !== "Gateway error: 500") {
    failures.push("an error frame did not update the bar text");
  }
  if (!statusText.classList.contains("status-bar__text--error")) {
    failures.push("an error frame did not style the bar text");
  }
  emitStatus({ label: "Ready", description: "idle" });
  if (statusText.classList.contains("status-bar__text--error")) {
    failures.push("the error styling did not clear on the next info frame");
  }
}

// Progress frames: a non-null progress renders the bar in the slot at the
// frame's fraction and hides the LED; a null progress removes the bar and
// restores the LED. Debug frames never touch the slot.
if (progressEl && ledEl) {
  emitStatus({
    label: "Downloading model",
    description: "1 of 4",
    activity: "general",
    progress: { current: 1, total: 4 },
  });
  if (progressEl.hidden) failures.push("a progress frame did not reveal the progress bar");
  if (progressEl.value !== 1 || progressEl.max !== 4) {
    failures.push(`progress bar shows ${progressEl.value}/${progressEl.max}, expected 1/4`);
  }
  if (!ledEl.hidden) failures.push("the LED did not hide while progress is showing");
  emitStatus({
    label: "Downloading model",
    description: "2 of 4",
    activity: "general",
    progress: { current: 2, total: 4 },
  });
  emitStatus({ label: "per-delta pulse", severity: "debug", activity: "generating" });
  if (progressEl.hidden || progressEl.value !== 2) {
    failures.push("a debug frame disturbed the progress bar");
  }
  emitStatus({ label: "Download complete", description: "ready" });
  if (!progressEl.hidden) failures.push("a null-progress frame did not hide the progress bar");
  if (ledEl.hidden) failures.push("the LED did not return when progress cleared");
}

// The activity LED: generating pulses light it green, thinking pulses
// amber, and green wins while both are lit inside one pulse window. Debug
// frames pulse it too. After the window the LED returns to its idle lens.
// The pulse window defaults to 250ms here (jsdom loads no stylesheet), so
// 400ms of silence guarantees decay.
if (ledEl) {
  const ledLit = () =>
    ledEl.classList.contains("status-bar__led--generating") ||
    ledEl.classList.contains("status-bar__led--thinking");
  // Let any pulse from the earlier sections decay before asserting idle.
  await new Promise((resolve) => setTimeout(resolve, 400));
  if (ledLit()) failures.push("the LED did not return to idle after the pulse window");
  emitStatus({ label: "delta", severity: "debug", activity: "generating" });
  if (!ledEl.classList.contains("status-bar__led--generating")) {
    failures.push("generating activity did not light the LED green");
  }
  emitStatus({ label: "thinking", severity: "debug", activity: "thinking" });
  if (!ledEl.classList.contains("status-bar__led--generating")) {
    failures.push("green did not win while generating and thinking were both lit");
  }
  if (ledEl.classList.contains("status-bar__led--thinking")) {
    failures.push("the thinking modifier applied while generating was lit");
  }
  await new Promise((resolve) => setTimeout(resolve, 400));
  if (ledLit()) failures.push("the LED stayed lit past the pulse window");
  emitStatus({ label: "thinking", severity: "debug", activity: "thinking" });
  if (!ledEl.classList.contains("status-bar__led--thinking")) {
    failures.push("thinking activity did not light the LED amber");
  }
  await new Promise((resolve) => setTimeout(resolve, 400));
  if (ledLit()) failures.push("the LED stayed lit past the pulse window");
}

// The REC badge: present in the status bar, idle at boot, lit while the
// mic records, and cleared when the voice socket drops.
const recEl = window.document.querySelector(".status-bar__rec");
if (!recEl) {
  failures.push("status bar REC badge missing");
} else if (mic) {
  if (recEl.classList.contains("status-bar__rec--active")) {
    failures.push("the REC badge must start idle");
  }
  mic.click();
  const recDeadline = Date.now() + 5000;
  while (!recEl.classList.contains("status-bar__rec--active") && Date.now() < recDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!recEl.classList.contains("status-bar__rec--active")) {
    failures.push("starting voice capture did not light the REC badge");
  }
  const voiceSocket = chatSockets.find((socket) => socket.url.endsWith("/voice"));
  if (!voiceSocket) {
    failures.push("no /voice socket was opened");
  } else {
    voiceSocket.onclose?.();
    if (recEl.classList.contains("status-bar__rec--active")) {
      failures.push("a dropped voice socket did not clear the REC badge");
    }
  }
}

// Disconnect recovery: a dropped /ws socket resets the bar to its
// reconnecting state, and the backoff opens a replacement socket (the
// first retry waits one second).
const persistentSocket = chatSockets.find((socket) => socket.url.endsWith("/ws"));
if (persistentSocket && statusText) {
  const socketCount = chatSockets.length;
  persistentSocket.onclose?.();
  if (statusText.textContent !== "Reconnecting...") {
    failures.push("a dropped /ws socket did not reset the status bar");
  }
  const reconnectDeadline = Date.now() + 5000;
  while (chatSockets.length === socketCount && Date.now() < reconnectDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (chatSockets.length === socketCount) {
    failures.push("no replacement /ws socket opened after the reconnect backoff");
  } else if (!chatSockets[chatSockets.length - 1].url.endsWith("/ws")) {
    failures.push("the reconnect opened a socket that is not /ws");
  }
}

if (failures.length > 0) {
  console.error(`smoke test failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("smoke test passed: the bundled app mounts the chat UI and answers a message");
// The voice-status auto-hide timer outlives the assertions; exit rather
// than wait it out.
process.exit(0);
