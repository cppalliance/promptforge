// Smoke test: loads dist/index.html into jsdom, imports the bundled
// dist/app.js, and asserts the chat UI mounts without throwing. Guards the
// DOM contract between index.html and the vendored murm-ui (its components
// throw when a required class is missing). Run after `npm run build`:
// `npm test`.
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
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};
// No model catalog answers in the test; loadModels falls into its error
// path, which only touches the picker.
window.fetch = () => Promise.reject(new Error("no server in the smoke test"));

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

// The bundle mounts ChatUI on .mur-app: a successful mount leaves the murm
// structure intact and renders the empty-chat state.
const app = window.document.querySelector(".mur-app");
const history = window.document.querySelector(".mur-chat-history");
const input = window.document.querySelector(".mur-chat-input");
const send = window.document.querySelector(".mur-send-btn");
const mic = window.document.querySelector(".mic-button");

const failures = [];
if (!app) failures.push(".mur-app missing");
if (!history) failures.push(".mur-chat-history missing");
if (!input) failures.push(".mur-chat-input missing");
if (!send) failures.push(".mur-send-btn missing");
if (!mic) failures.push("voice plugin did not insert the mic button");
if (app && !app.classList.contains("mur-chat-empty")) {
  failures.push("fresh mount is not in the empty-chat state");
}

if (failures.length > 0) {
  console.error(`smoke test failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("smoke test passed: the bundled app mounts the chat UI");
