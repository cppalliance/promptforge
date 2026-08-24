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
// A scripted WebSocket stands in for the server's /ws route. It must live on
// globalThis: the bundle calls the global `WebSocket`, not `window.WebSocket`.
// Each chat frame sent to the socket is captured and answered with two delta
// frames and a done frame, scheduled in order so the provider's round-trip
// runs.
const chatSockets = [];
class FakeWebSocket {
  constructor(url) {
    this.url = url;
    chatSockets.push(this);
    queueMicrotask(() => this.onopen?.());
  }
  send(data) {
    this.chatFrame = JSON.parse(data);
    const frames = [
      { type: "delta", content: "Hello" },
      { type: "delta", content: " back" },
      { type: "done" },
    ];
    for (const frame of frames) {
      queueMicrotask(() => this.onmessage?.({ data: JSON.stringify(frame) }));
    }
  }
  close() {}
}
globalThis.WebSocket = FakeWebSocket;
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
const mic = window.document.querySelector(".mic-button");
const statusBar = window.document.querySelector(".status-bar");

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
    if (!socket.url.endsWith("/ws")) failures.push(`chat socket opened the wrong URL: ${socket.url}`);
    const request = socket.chatFrame;
    if (!request) {
      failures.push("no chat frame was sent on the socket");
    } else {
      if (request.type !== "chat") failures.push("the frame is not a chat frame");
      if (request.model !== "test-model") failures.push("chat frame carried the wrong model");
      if ("stream" in request) failures.push("chat frame must not carry a stream flag");
      const first = request.messages?.[0];
      if (!first || first.role !== "user" || first.content !== "Hello?") {
        failures.push("chat frame messages are not the OpenAI shape");
      }
    }
  }
}

if (failures.length > 0) {
  console.error(`smoke test failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("smoke test passed: the bundled app mounts the chat UI and answers a message");
