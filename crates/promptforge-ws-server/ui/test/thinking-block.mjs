// Unit test for the three-state thinking block
// (src/chat/plugins/thinking/thinking-plugin.ts). Bundles the TS module
// with esbuild (CSS import stripped), imports it via a data URL, and drives
// it against jsdom. Covers: grace-loader timing and suppression, preview
// opening on the first reasoning delta, the four-line preview cap with
// internal scroll, auto-pin disengage/re-engage, auto-collapse on the first
// content token with scroll-lock, the sticky manual toggle, and full
// expansion.
// Run: node test/thinking-block.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.DOMParser = window.DOMParser;
globalThis.NodeFilter = window.NodeFilter;
globalThis.Node = window.Node;
globalThis.HTMLElement = window.HTMLElement;

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "chat", "plugins", "thinking", "thinking-plugin.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  loader: { ".css": "empty" },
  logLevel: "silent",
});
const code = bundle.outputFiles[0].text;
const mod = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
const { ThinkingPlugin } = mod;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function reasoningBlock(text, id = "b1") {
  return { id, type: "reasoning", text };
}

// Minimal ChatEngine stand-in: a selectable state plus onChange wiring, the
// only surface the plugin's grace loader touches.
function createFakeEngine() {
  let state = { generatingMessageId: null, messages: [] };
  const listeners = [];
  return {
    get state() {
      return state;
    },
    onChange(selector, listener) {
      listeners.push({ selector, listener });
      return () => {};
    },
    setState(patch) {
      const prev = state;
      state = { ...state, ...patch };
      for (const { selector, listener } of listeners) {
        const before = selector(prev);
        const after = selector(state);
        if (before !== after) listener(after);
      }
    },
  };
}

// --- Grace-period loader ----------------------------------------------------

{
  const plugin = ThinkingPlugin();
  const engine = createFakeEngine();
  const container = window.document.createElement("div");
  const messageEl = window.document.createElement("div");
  messageEl.className = "mur-message mur-message-assistant";
  container.appendChild(messageEl);
  plugin.onMount({ engine, container });

  engine.setState({ generatingMessageId: "m1", messages: [{ id: "m1", role: "assistant", blocks: [] }] });
  await sleep(250);
  check("grace row absent before 500ms", !container.querySelector(".mur-think-grace"));
  await sleep(400);
  const grace = container.querySelector(".mur-think-grace");
  check("grace row appears after ~500ms", !!grace);
  check("grace row reads the streaming label", grace?.textContent?.includes("Planning next moves..."));
  check("grace row announces as a status", grace?.getAttribute("role") === "status");

  engine.setState({ generatingMessageId: null });
  check("grace row removed when generation ends", !container.querySelector(".mur-think-grace"));

  // A reasoning block arriving inside the grace window suppresses the loader.
  engine.setState({ generatingMessageId: "m2", messages: [{ id: "m2", role: "assistant", blocks: [] }] });
  const earlyBlock = window.document.createElement("div");
  messageEl.appendChild(earlyBlock);
  plugin.onBlockRender(reasoningBlock("draft", "b2"), earlyBlock, true);
  await sleep(600);
  check("no grace row when reasoning arrives before 500ms", !container.querySelector(".mur-think-grace"));

  // A reasoning block arriving after the loader showed removes it.
  engine.setState({ generatingMessageId: null });
  engine.setState({ generatingMessageId: "m3", messages: [{ id: "m3", role: "assistant", blocks: [] }] });
  await sleep(600);
  check("grace row appears again for a slow stream", !!container.querySelector(".mur-think-grace"));
  const lateBlock = window.document.createElement("div");
  messageEl.appendChild(lateBlock);
  plugin.onBlockRender(reasoningBlock("draft", "b3"), lateBlock, true);
  check("grace row removed when the real block arrives", !container.querySelector(".mur-think-grace"));

  // A text block already present when the timer fires suppresses the loader.
  engine.setState({ generatingMessageId: null });
  engine.setState({
    generatingMessageId: "m4",
    messages: [{ id: "m4", role: "assistant", blocks: [{ id: "t4", type: "text", text: "hi" }] }],
  });
  await sleep(600);
  check("no grace row when content arrived before the timer fired", !container.querySelector(".mur-think-grace"));

  // A text block arriving while the loader shows removes it.
  engine.setState({ generatingMessageId: null });
  engine.setState({ generatingMessageId: "m5", messages: [{ id: "m5", role: "assistant", blocks: [] }] });
  await sleep(600);
  check("grace row showing for a slow text-first stream", !!container.querySelector(".mur-think-grace"));
  const textBlock = window.document.createElement("div");
  messageEl.appendChild(textBlock);
  plugin.onBlockRender({ id: "t5", type: "text", text: "answer" }, textBlock, true);
  check("grace row removed when the first text block renders", !container.querySelector(".mur-think-grace"));

  plugin.destroy();
}

// --- Preview opens on the first reasoning delta ------------------------------

const plugin = ThinkingPlugin();
const container = window.document.createElement("div");
plugin.onBlockRender(reasoningBlock("step one"), container, true);

const btn = container.querySelector(".mur-think-toggle");
const content = container.querySelector(".mur-think-content");
const label = container.querySelector(".mur-think-label");
const live = container.querySelector(".mur-think-sr-only");

check("toggle is a real button", btn?.tagName === "BUTTON");
check("aria-controls points at the content id", !!content && btn?.getAttribute("aria-controls") === content.id);
check("preview opens on first delta", btn?.getAttribute("aria-expanded") === "true" && content?.hidden === false);
check("preview mode class applied", content?.classList.contains("mur-think-content--preview"));
check("label reads Planning next moves while streaming", label?.textContent === "Planning next moves...");
check("shimmer class while streaming", label?.classList.contains("mur-think-label--streaming"));
check("state transition announced in the live region", live?.textContent === "Planning next moves...");
check("reasoning text rendered into the preview", content?.textContent?.includes("step one"));

// --- Four-line cap with internal scroll (stylesheet contract) ----------------

const css = await readFile(
  path.join(uiDir, "..", "src", "chat", "plugins", "thinking", "thinking.css"),
  "utf8",
);
const previewRule = /\.mur-think-content\.mur-think-content--preview\s*\{([^}]*)\}/.exec(css);
check("preview cap rule exists", !!previewRule);
check("preview capped at roughly four lines", /max-height:\s*6\.4rem/.test(previewRule?.[1] ?? ""));
check("preview scrolls internally", /overflow-y:\s*auto/.test(previewRule?.[1] ?? ""));
check("shimmer animates background-position", /@keyframes mur-think-shimmer[\s\S]*?background-position/.test(css));
check(
  "shimmer is static under prefers-reduced-motion",
  /prefers-reduced-motion:\s*reduce[\s\S]*?mur-think-label--streaming[\s\S]*?animation:\s*none/.test(css),
);

// --- Auto-pin disengage / re-engage ------------------------------------------

// jsdom has no layout engine; stub line-based metrics so scroll math runs.
let storedScrollTop = 0;
Object.defineProperty(content, "scrollHeight", { configurable: true, get: () => 1000 });
Object.defineProperty(content, "clientHeight", { configurable: true, get: () => 100 });
Object.defineProperty(content, "scrollTop", {
  configurable: true,
  get: () => storedScrollTop,
  set: (value) => {
    storedScrollTop = value;
  },
});

plugin.onBlockRender(reasoningBlock("step one\nstep two"), container, true);
check("preview pinned to the newest line", storedScrollTop === 1000);

storedScrollTop = 400;
content.dispatchEvent(new window.Event("scroll"));
plugin.onBlockRender(reasoningBlock("step one\nstep two\nstep three"), container, true);
check("scroll-up disengages auto-pin", storedScrollTop === 400);

storedScrollTop = 900; // scrollHeight - clientHeight: back at the bottom
content.dispatchEvent(new window.Event("scroll"));
plugin.onBlockRender(reasoningBlock("step one\nstep two\nstep three\nstep four"), container, true);
check("scrolling back to the bottom re-engages auto-pin", storedScrollTop === 1000);

// --- Auto-collapse on the first content token, with scroll-lock --------------

const scrollArea = window.document.createElement("div");
scrollArea.className = "mur-chat-scroll-area";
const streamContainer = window.document.createElement("div");
scrollArea.appendChild(streamContainer);

plugin.onBlockRender(reasoningBlock("draft", "b9"), streamContainer, true);
check(
  "second block streams into preview",
  streamContainer.querySelector(".mur-think-content")?.hidden === false,
);
scrollArea.scrollTop = 420;
plugin.onBlockRender(reasoningBlock("draft", "b9"), streamContainer, false);

const streamBtn = streamContainer.querySelector(".mur-think-toggle");
const streamContent = streamContainer.querySelector(".mur-think-content");
const streamLabel = streamContainer.querySelector(".mur-think-label");
check("auto-collapses on the first content token", streamContent?.hidden === true);
check("aria-expanded flips to false", streamBtn?.getAttribute("aria-expanded") === "false");
check("label returns to Thinking", streamLabel?.textContent === "Thinking");
check("shimmer removed after streaming", !streamLabel?.classList.contains("mur-think-label--streaming"));
check(
  "collapse announced in the live region",
  streamContainer.querySelector(".mur-think-sr-only")?.textContent === "Thinking",
);
await sleep(50); // let the rAF scroll-lock land
check("scroll-lock restores the feed position", scrollArea.scrollTop === 420);

// --- Sticky manual toggle ------------------------------------------------------

const manualContainer = window.document.createElement("div");
plugin.onBlockRender(reasoningBlock("m1", "b10"), manualContainer, true);
const manualBtn = manualContainer.querySelector(".mur-think-toggle");
const manualContent = manualContainer.querySelector(".mur-think-content");

manualBtn.click();
check("manual toggle expands from preview", manualContent?.classList.contains("mur-think-content--expanded"));
plugin.onBlockRender(reasoningBlock("m1 longer", "b10"), manualContainer, false);
check("manual expansion survives the first content token", manualContent?.hidden === false);
check("still expanded after stream end", manualContent?.classList.contains("mur-think-content--expanded"));

manualBtn.click();
check("second click collapses", manualContent?.hidden === true);
plugin.onBlockRender(reasoningBlock("m1 longer still", "b10"), manualContainer, true);
check("manual collapse is sticky across later deltas", manualContent?.hidden === true);

// --- Full expansion -------------------------------------------------------------

const fullContainer = window.document.createElement("div");
const longText = "line one\nline two\nline three\nline four\nline five\nline six";
plugin.onBlockRender(reasoningBlock(longText, "b11"), fullContainer, true);
fullContainer.querySelector(".mur-think-toggle").click();
const fullContent = fullContainer.querySelector(".mur-think-content");
check("expanded mode class applied", fullContent?.classList.contains("mur-think-content--expanded"));
check("expanded mode drops the preview cap class", !fullContent?.classList.contains("mur-think-content--preview"));
check("full thinking rendered", fullContent?.textContent?.includes("line six"));
check("expanded aria state", fullContainer.querySelector(".mur-think-toggle")?.getAttribute("aria-expanded") === "true");

if (failures.length > 0) {
  console.error(`thinking-block: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("thinking-block: all assertions passed");
