// Unit test for the three-state thinking block
// (src/chat/plugins/thinking/thinking-plugin.ts). Bundles the TS module
// with esbuild (CSS import stripped), imports it via a data URL, and drives
// it against jsdom. Covers: dot-free prefill on generation start (including
// the render race where the feed creates the message element after the
// selector fires, and targeting the generating message by id),
// the first-token transition from "Planning next moves" to the "Thinking"
// toggle, the four-line preview cap with internal scroll, auto-pin
// disengage/re-engage, auto-collapse on the first content token with
// scroll-lock, durable completed thinking, repeated expand-collapse
// toggling before and after completion, and message-node's suppression of
// the generic three-dot loader when a plugin owns the empty loading state.
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
// only surface the plugin's prefill indicator touches.
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

// --- Prefill indicator ----------------------------------------------------------

// The attach defers past the current render pass (microtask plus a few
// frames); a short sleep lets it land in jsdom.
const flushPrefill = () => sleep(60);

{
  const plugin = ThinkingPlugin();
  const engine = createFakeEngine();
  const container = window.document.createElement("div");
  const addMessageEl = (id) => {
    const messageEl = window.document.createElement("div");
    messageEl.className = "mur-message mur-message-assistant";
    messageEl.dataset.messageId = id;
    container.appendChild(messageEl);
    return messageEl;
  };
  plugin.onMount({ engine, container });

  // An earlier completed turn: the prefill must never attach to it.
  const previousTurn = addMessageEl("m0");

  // The real feed renders the message element on the hot pass, after the
  // selector notification the prefill listens to; creating the element
  // after setState reproduces that order (the row used to race and lose).
  engine.setState({ generatingMessageId: "m1", messages: [{ id: "m1", role: "assistant", blocks: [] }] });
  const generatingTurn = addMessageEl("m1");
  await flushPrefill();
  const prefill = container.querySelector(".mur-think-prefill");
  check("prefill row appears on generation start", !!prefill);
  check("prefill attaches to the generating message", prefill?.parentElement === generatingTurn);
  check("prefill never lands on an earlier turn", !previousTurn.querySelector(".mur-think-prefill"));
  check("prefill label is exactly Planning next moves", prefill?.textContent === "Planning next moves");
  check("prefill label has no ellipsis", !prefill?.textContent?.includes("..."));
  check("prefill row has no three-dot loader", !prefill?.querySelector(".mur-loading-dot"));
  check("prefill row announces as a status", prefill?.getAttribute("role") === "status");
  check("prefill label carries the shimmer class", !!prefill?.querySelector(".mur-think-label--prefill"));
  await sleep(600);
  check("prefill row is not a delayed loader", container.querySelectorAll(".mur-think-prefill").length === 1);

  engine.setState({ generatingMessageId: null });
  check("prefill row removed when generation ends", !container.querySelector(".mur-think-prefill"));

  // A reasoning block arriving right after generation start replaces prefill.
  engine.setState({ generatingMessageId: "m2", messages: [{ id: "m2", role: "assistant", blocks: [] }] });
  const secondTurn = addMessageEl("m2");
  await flushPrefill();
  check("prefill row returns for the next generation", !!container.querySelector(".mur-think-prefill"));
  const earlyBlock = window.document.createElement("div");
  secondTurn.appendChild(earlyBlock);
  plugin.onBlockRender(reasoningBlock("draft", "b2"), earlyBlock, true);
  check("first reasoning token removes the prefill row", !container.querySelector(".mur-think-prefill"));

  // A message that already has content never shows prefill.
  engine.setState({ generatingMessageId: null });
  engine.setState({
    generatingMessageId: "m4",
    messages: [{ id: "m4", role: "assistant", blocks: [{ id: "t4", type: "text", text: "hi" }] }],
  });
  addMessageEl("m4");
  await flushPrefill();
  check("no prefill row when content already exists", !container.querySelector(".mur-think-prefill"));

  // A text block arriving while prefill shows removes it.
  engine.setState({ generatingMessageId: null });
  engine.setState({ generatingMessageId: "m5", messages: [{ id: "m5", role: "assistant", blocks: [] }] });
  const fifthTurn = addMessageEl("m5");
  await flushPrefill();
  check("prefill row showing for a text-first stream", !!container.querySelector(".mur-think-prefill"));
  const textBlock = window.document.createElement("div");
  fifthTurn.appendChild(textBlock);
  plugin.onBlockRender({ id: "t5", type: "text", text: "answer" }, textBlock, true);
  check("prefill row removed when the first text block renders", !container.querySelector(".mur-think-prefill"));

  plugin.destroy();
}

// --- First-token transition: prefill becomes the Thinking toggle ---------------

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
check("label transitions to Thinking on the first token", label?.textContent === "Thinking");
check("no shimmer class on the streaming Thinking label", !label?.classList.contains("mur-think-label--prefill"));
check("stream start announced in the live region", live?.textContent === "Thinking");
check("reasoning text rendered into the preview", content?.textContent?.includes("step one"));

// --- Four-line cap with internal scroll (stylesheet contract) ------------------

const css = await readFile(
  path.join(uiDir, "..", "src", "chat", "plugins", "thinking", "thinking.css"),
  "utf8",
);
const previewRule = /\.mur-think-content\.mur-think-content--preview\s*\{([^}]*)\}/.exec(css);
check("preview cap rule exists", !!previewRule);
check("preview capped at roughly four lines", /max-height:\s*6\.4rem/.test(previewRule?.[1] ?? ""));
check("preview scrolls internally", /overflow-y:\s*auto/.test(previewRule?.[1] ?? ""));
check("shimmer animates background-position", /@keyframes mur-think-shimmer[\s\S]*?background-position/.test(css));
check("shimmer is scoped to the prefill label", /\.mur-think-label--prefill\s*\{[\s\S]*?mur-think-shimmer/.test(css));
check("no streaming-label shimmer rule remains", !/mur-think-label--streaming/.test(css));
check(
  "shimmer is static under prefers-reduced-motion",
  /prefers-reduced-motion:\s*reduce[\s\S]*?mur-think-label--prefill[\s\S]*?animation:\s*none/.test(css),
);

// --- Auto-pin disengage / re-engage ---------------------------------------------

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

// --- Repeated toggling during streaming ------------------------------------------

btn.click();
check("click during streaming expands", content.classList.contains("mur-think-content--expanded"));
btn.click();
check("second click during streaming collapses", content.hidden === true);
btn.click();
check("third click during streaming expands again", content.hidden === false);

// --- Auto-collapse on the first content token, with scroll-lock -------------------

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
check("label stays Thinking after streaming", streamLabel?.textContent === "Thinking");
check(
  "completion announced in the live region",
  streamContainer.querySelector(".mur-think-sr-only")?.textContent === "Thinking complete",
);
await sleep(50); // let the rAF scroll-lock land
check("scroll-lock restores the feed position", scrollArea.scrollTop === 420);

// --- Durable completed thinking ----------------------------------------------------

check("toggle survives completion", !!streamContainer.querySelector(".mur-think-toggle"));
check("completed reasoning content is preserved", streamContent?.textContent?.includes("draft"));

streamBtn.click();
check("click after completion expands the preserved reasoning", streamContent?.hidden === false);
check("expanded reasoning still intact", streamContent?.textContent?.includes("draft"));
streamBtn.click();
check("second click after completion rolls the block back up", streamContent?.hidden === true);
streamBtn.click();
check("toggle remains repeatable after completion", streamContent?.hidden === false);

// --- Sticky manual toggle -------------------------------------------------------------

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

// --- Full expansion ---------------------------------------------------------------------

const fullContainer = window.document.createElement("div");
const longText = "line one\nline two\nline three\nline four\nline five\nline six";
plugin.onBlockRender(reasoningBlock(longText, "b11"), fullContainer, true);
fullContainer.querySelector(".mur-think-toggle").click();
const fullContent = fullContainer.querySelector(".mur-think-content");
check("expanded mode class applied", fullContent?.classList.contains("mur-think-content--expanded"));
check("expanded mode drops the preview cap class", !fullContent?.classList.contains("mur-think-content--preview"));
check("full thinking rendered", fullContent?.textContent?.includes("line six"));
check("expanded aria state", fullContainer.querySelector(".mur-think-toggle")?.getAttribute("aria-expanded") === "true");

// --- Message-node dot suppression when a plugin owns the loading state ----------

const nodeBundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "chat", "components", "message-node.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  loader: { ".css": "empty" },
  logLevel: "silent",
});
const nodeCode = nodeBundle.outputFiles[0].text;
const nodeMod = await import(`data:text/javascript;base64,${Buffer.from(nodeCode).toString("base64")}`);
const { MessageNode } = nodeMod;

const emptyAssistant = { id: "mn1", role: "assistant", blocks: [] };

const bareNode = new MessageNode(emptyAssistant, { plugins: [] });
bareNode.update(emptyAssistant, true, null, [emptyAssistant]);
check("generic dots render without an owning plugin", !!bareNode.el.querySelector(".mur-message-loading"));
bareNode.destroy();

const ownedNode = new MessageNode(emptyAssistant, {
  plugins: [{ name: "thinking", ownsEmptyLoadingState: true }],
});
ownedNode.update(emptyAssistant, true, null, [emptyAssistant]);
check("plugin ownership suppresses the generic dots", !ownedNode.el.querySelector(".mur-message-loading"));
ownedNode.destroy();

if (failures.length > 0) {
  console.error(`thinking-block: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("thinking-block: all assertions passed");
