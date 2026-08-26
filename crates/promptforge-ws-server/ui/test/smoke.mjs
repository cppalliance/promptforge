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
    const frames = [
      { type: "delta", content: "Hello", id: frame.id },
      { type: "delta", content: " back [docs](https://example.com/)", id: frame.id },
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
// Node ships its own Event and CustomEvent globals, so the copy loop skips
// them - but events the bundle dispatches into the jsdom document must be
// jsdom-realm instances: jsdom's dispatchEvent rejects Node's Event with
// "parameter 1 is not of type 'Event'".
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;
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
if (window.document.querySelector(".voice-status")) {
  failures.push("a .voice-status element exists after the voice plugin mounted");
}
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
  // Sanitized anchors open externally: the sanitizer must stamp
  // target="_blank" and rel="noopener" on every rendered link.
  const replyLink = history.querySelector('a[href="https://example.com/"]');
  if (!replyLink) {
    failures.push("the assistant reply's markdown link did not render as an anchor");
  } else {
    if (replyLink.getAttribute("target") !== "_blank") {
      failures.push('a sanitized anchor is missing target="_blank"');
    }
    if (replyLink.getAttribute("rel") !== "noopener") {
      failures.push('a sanitized anchor is missing rel="noopener"');
    }
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

// Composer auto-grow: an interim transcript rewrites the textarea
// programmatically, and the box must grow to fit it. The voice path
// notifies murm-ui's Input through a dispatched "input" event; in jsdom
// (no CSS global in Node) murm-ui takes its adjustHeight path, which the
// scrollHeight shim above turns into an observable inline height.
if (mic && input) {
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let takeSocket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      takeSocket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!takeSocket) {
    failures.push("the second mic click did not open a /voice socket with a message listener");
  } else {
    const interimText = "line one\nline two\nline three";
    const heightBefore = parseFloat(input.style.height) || 0;
    takeSocket.onmessage({
      data: JSON.stringify({ type: "interim", committed: interimText, tentative: "" }),
    });
    const heightAfter = parseFloat(input.style.height) || 0;
    if (input.value !== interimText) {
      failures.push("the interim transcript did not land in the composer");
    }
    if (!(heightAfter > heightBefore)) {
      failures.push(
        `the composer did not grow on a multiline interim (was ${input.style.height || "unset"})`,
      );
    }
    takeSocket.onclose?.();
  }
}

// Committed/tentative interims: the textarea shows committed + tentative,
// joined with a space only when the committed prefix does not already end
// in whitespace. Committed is append-only within a take, so the display
// follows the server unconditionally - a shorter tentative never shrinks
// the text while committed keeps growing.
if (mic && input) {
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let takeSocket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      takeSocket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!takeSocket) {
    failures.push("the third mic click did not open a /voice socket with a message listener");
  } else {
    const sendInterim = (committed, tentative) =>
      takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed, tentative }) });
    sendInterim("One two.", "three");
    if (input.value !== "One two. three") {
      failures.push(`committed+tentative did not join with a space: "${input.value}"`);
    }
    sendInterim("One two. three four.", "");
    if (input.value !== "One two. three four.") {
      failures.push(`a grown committed prefix did not land verbatim: "${input.value}"`);
    }
    const grownLength = input.value.length;
    sendInterim("One two. three four. five six.", "se");
    if (input.value !== "One two. three four. five six. se") {
      failures.push(`a shorter tentative with grown committed mis-rendered: "${input.value}"`);
    }
    if (input.value.length <= grownLength) {
      failures.push("the text shrank while committed kept growing");
    }
    sendInterim("One two. three four. five six. ", "seven");
    if (input.value !== "One two. three four. five six. seven") {
      failures.push(`a trailing-whitespace committed prefix gained a double space: "${input.value}"`);
    }
    sendInterim("", "fresh start");
    if (input.value !== "fresh start") {
      failures.push(`an empty committed prefix gained a leading space: "${input.value}"`);
    }
    takeSocket.onclose?.();
  }
}

// Insert-at-cursor: with "ab" in the textarea and the cursor between a and
// b, record an interim "X"; assert "aXb" and the cursor sits after X.
if (mic && input) {
  input.value = "ab";
  input.setSelectionRange(1, 1);
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let takeSocket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      takeSocket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!takeSocket) {
    failures.push("insert-at-cursor: mic click did not open a /voice socket");
  } else {
    if (!input.readOnly) {
      failures.push("insert-at-cursor: input.readOnly must be true during the take");
    }
    takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "X", tentative: "" }) });
    if (input.value !== "aXb") {
      failures.push(`insert-at-cursor: expected "aXb", got "${input.value}"`);
    }
    if (input.selectionStart !== 2) {
      failures.push(`insert-at-cursor: cursor expected at 2, got ${input.selectionStart}`);
    }
    takeSocket.onmessage({ data: JSON.stringify({ type: "final", text: "Y" }) });
    if (input.value !== "aYb") {
      failures.push(`insert-at-cursor final: expected "aYb", got "${input.value}"`);
    }
    if (input.readOnly) {
      failures.push("insert-at-cursor: readOnly not cleared after final");
    }
    // FakeWebSocket doesn't auto-fire onclose; trigger it so voice state resets.
    takeSocket.onclose?.();
  }
}

// Selection replacement: with "ab" fully selected, record an interim "X";
// assert the box shows "X".
if (mic && input) {
  input.value = "ab";
  input.setSelectionRange(0, 2);
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let takeSocket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      takeSocket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!takeSocket) {
    failures.push("selection-replace: mic click did not open a /voice socket");
  } else {
    takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "X", tentative: "" }) });
    if (input.value !== "X") {
      failures.push(`selection-replace: expected "X", got "${input.value}"`);
    }
    takeSocket.onclose?.();
  }
}

// Discard on send: start recording, submit a chat, assert REC cleared, the
// voice socket was closed, and a late final does not write into the textarea.
if (mic && input && form && recEl) {
  input.value = "";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let discardSocket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      discardSocket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!discardSocket) {
    failures.push("discard-on-send: mic click did not open a /voice socket");
  } else {
    discardSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: "hello", tentative: "" }) });
    if (!recEl.classList.contains("status-bar__rec--active")) {
      failures.push("discard-on-send: REC badge not lit before submit");
    }
    input.value = "send this";
    input.dispatchEvent(new window.Event("input", { bubbles: true }));
    form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
    const submitDeadline = Date.now() + 2000;
    while (recEl.classList.contains("status-bar__rec--active") && Date.now() < submitDeadline) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (recEl.classList.contains("status-bar__rec--active")) {
      failures.push("discard-on-send: REC badge not cleared after submit");
    }
    if (discardSocket.readyState !== FakeWebSocket.CLOSED) {
      failures.push("discard-on-send: voice socket was not closed");
    }
    if (input.readOnly) {
      failures.push("discard-on-send: readOnly not cleared after discard");
    }
    const valueBeforeLate = input.value;
    discardSocket.onmessage?.({ data: JSON.stringify({ type: "final", text: "LATE FINAL" }) });
    if (input.value !== valueBeforeLate) {
      failures.push("discard-on-send: a late final frame wrote into the textarea after discard");
    }
  }
}

// readOnly during takes: assert readOnly is true during a take and false
// after a discard.
if (mic && input) {
  input.value = "prefix";
  input.setSelectionRange(6, 6);
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let takeSocket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      takeSocket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!takeSocket) {
    failures.push("readOnly-take: mic click did not open a /voice socket");
  } else {
    if (!input.readOnly) {
      failures.push("readOnly-take: input.readOnly must be true during the take");
    }
    takeSocket.onmessage({ data: JSON.stringify({ type: "interim", committed: " world", tentative: "" }) });
    if (input.value !== "prefix world") {
      failures.push(`readOnly-take: expected "prefix world", got "${input.value}"`);
    }
    // Simulate stop via mic click (triggers stopVoice, then final arrives)
    mic.click();
    takeSocket.onmessage({ data: JSON.stringify({ type: "final", text: " world" }) });
    if (input.readOnly) {
      failures.push("readOnly-take: readOnly not cleared after final");
    }
    if (input.value !== "prefix world") {
      failures.push(`readOnly-take: final text wrong, got "${input.value}"`);
    }
  }
}

// Multi-take composition: first take inserts "hello" at end, second take
// inserts " world" at the new cursor position (after "hello").
if (mic && input) {
  input.value = "start";
  input.setSelectionRange(5, 5);
  const voiceSocketCount = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
  mic.click();
  const openDeadline = Date.now() + 5000;
  let take1Socket;
  while (Date.now() < openDeadline) {
    const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
    if (voiceSockets.length > voiceSocketCount && typeof voiceSockets.at(-1).onmessage === "function") {
      take1Socket = voiceSockets.at(-1);
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (!take1Socket) {
    failures.push("multi-take: first mic click did not open a /voice socket");
  } else {
    take1Socket.onmessage({ data: JSON.stringify({ type: "final", text: " hello" }) });
    if (input.value !== "start hello") {
      failures.push(`multi-take: after take 1 expected "start hello", got "${input.value}"`);
    }
    take1Socket.onclose?.();
    // Second take: cursor should be at position 11 ("start hello|")
    const voiceSocketCount2 = chatSockets.filter((socket) => socket.url.endsWith("/voice")).length;
    mic.click();
    const openDeadline2 = Date.now() + 5000;
    let take2Socket;
    while (Date.now() < openDeadline2) {
      const voiceSockets = chatSockets.filter((socket) => socket.url.endsWith("/voice"));
      if (voiceSockets.length > voiceSocketCount2 && typeof voiceSockets.at(-1).onmessage === "function") {
        take2Socket = voiceSockets.at(-1);
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!take2Socket) {
      failures.push("multi-take: second mic click did not open a /voice socket");
    } else {
      take2Socket.onmessage({ data: JSON.stringify({ type: "final", text: " world" }) });
      if (input.value !== "start hello world") {
        failures.push(`multi-take: after take 2 expected "start hello world", got "${input.value}"`);
      }
      if (input.readOnly) {
        failures.push("multi-take: readOnly not cleared after second take");
      }
      take2Socket.onclose?.();
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
// Pending timers (the voice stop grace window, the status-bar LED pulse)
// outlive the assertions; exit rather than wait them out.
process.exit(0);
