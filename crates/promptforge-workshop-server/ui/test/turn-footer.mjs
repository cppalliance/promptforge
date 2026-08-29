// Unit test for the model-turn footer (src/chat/components/turn-footer.ts)
// as mounted by feed-node.ts. Bundles the TS modules with esbuild, imports
// them via data URLs, and drives them against jsdom. Covers: one footer per
// turn for plain assistant messages and grouped agent runs, clipboard copy
// with checkmark feedback, inert Fork, relative-time text, tooltip content
// (absolute time + run duration), footer persistence across work-segment
// collapse, accessibility labels, and timer cleanup on destroy.
// Run: node test/turn-footer.mjs
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
// jsdom has no clipboard; stub it and capture writes. Node 24 exposes
// globalThis.navigator as a getter-only accessor, so redefine it.
let copiedText = null;
Object.defineProperty(window.navigator, "clipboard", {
  configurable: true,
  value: {
    writeText: async (text) => {
      copiedText = text;
    },
  },
});
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: window.navigator,
});

// Track outstanding window timers so destroy() cleanup is observable.
const pendingTimers = new Set();
const realSetTimeout = window.setTimeout.bind(window);
const realClearTimeout = window.clearTimeout.bind(window);
window.setTimeout = (handler, timeout, ...args) => {
  const id = realSetTimeout(() => {
    pendingTimers.delete(id);
    if (typeof handler === "function") handler(...args);
  }, timeout);
  pendingTimers.add(id);
  return id;
};
window.clearTimeout = (id) => {
  pendingTimers.delete(id);
  realClearTimeout(id);
};

async function bundle(entry) {
  const result = await esbuild.build({
    entryPoints: [path.join(uiDir, "..", "src", "chat", entry)],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    loader: { ".css": "empty" },
    logLevel: "silent",
  });
  const code = result.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const feedItemsMod = await bundle(path.join("components", "feed-items.ts"));
const feedNodeMod = await bundle(path.join("components", "feed-node.ts"));
const formatMod = await bundle(path.join("utils", "format.ts"));
const { buildFeedItems } = feedItemsMod;
const { createFeedNode } = feedNodeMod;
const { formatRelativeTime } = formatMod;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

check("relative time under a minute is just now", formatRelativeTime(30_000) === "just now");
check("relative time clamps negative elapsed", formatRelativeTime(-1000) === "just now");
check("relative time minutes", formatRelativeTime(5 * 60_000) === "5m ago");
check("relative time hours", formatRelativeTime(3 * 3_600_000) === "3h ago");
check("relative time days", formatRelativeTime(2 * 24 * 3_600_000) === "2d ago");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const config = { plugins: [] };

function makeCtx(overrides = {}) {
  return {
    messages: [],
    generatingMessageId: null,
    error: null,
    onToggleWorkSegment: () => {},
    ...overrides,
  };
}

const NOW = Date.now();

// --- Plain assistant message ------------------------------------------------

const plainMessage = {
  id: "m1",
  role: "assistant",
  createdAt: NOW - 2 * 60_000,
  updatedAt: NOW - 2 * 60_000,
  blocks: [{ id: "t1", type: "text", text: "Hello world" }],
};

const plainNode = createFeedNode(plainMessage, config);
window.document.body.appendChild(plainNode.el);
plainNode.update(plainMessage, makeCtx({ messages: [plainMessage] }));

check("plain message type", plainNode.type === "message");
check(
  "plain assistant turn gets exactly one footer",
  plainNode.el.querySelectorAll(".mur-turn-footer").length === 1,
);
const plainFooter = plainNode.el.querySelector(".mur-turn-footer");
check("footer mounts after the message content", plainNode.el.lastElementChild === plainFooter);
check("footer visible for a completed turn", plainFooter.hidden === false);

const copyButton = plainFooter.querySelector('button[aria-label="Copy response"]');
const forkButton = plainFooter.querySelector('button[aria-label="Fork conversation"]');
check("copy button present with accessible name", !!copyButton);
check("fork button present with accessible name", !!forkButton);
check("fork button tooltip", forkButton?.title === "Fork conversation");

// Relative time text
const timeEl = plainFooter.querySelector("time");
check("semantic time element present", timeEl?.tagName === "TIME");
check("time element has dateTime", !!timeEl?.dateTime);
check("relative time text is compact minutes", timeEl?.textContent === "2m ago");

// Tooltip: absolute time, no duration for a plain message
const tooltip = plainFooter.querySelector(".mur-turn-footer-tooltip");
check("tooltip has role tooltip", tooltip?.getAttribute("role") === "tooltip");
check(
  "tooltip shows localized absolute time",
  tooltip?.querySelector(".mur-turn-footer-tooltip-time")?.textContent ===
    new Date(plainMessage.updatedAt).toLocaleString(),
);
check(
  "plain message tooltip has no duration line",
  !tooltip?.querySelector(".mur-turn-footer-tooltip-duration"),
);

// Copy behavior
copyButton.click();
await sleep(0);
check("clipboard receives the turn plain text", copiedText === "Hello world");
check(
  "copy button swaps to checkmark feedback",
  copyButton.classList.contains("mur-turn-footer-button--copied"),
);
await sleep(2100);
check(
  "copy icon restores after feedback window",
  !copyButton.classList.contains("mur-turn-footer-button--copied"),
);

// Inert fork
const beforeFork = window.document.body.innerHTML;
forkButton.click();
await sleep(0);
check("fork click is inert (no errors, no mutation)", window.document.body.innerHTML === beforeFork);
check("fork click does not touch the clipboard", copiedText === "Hello world");

// Hidden while generating
plainNode.update(plainMessage, makeCtx({ messages: [plainMessage], generatingMessageId: "m1" }));
check("footer hides while the turn is generating", plainFooter.hidden === true);
plainNode.update(plainMessage, makeCtx({ messages: [plainMessage] }));
check("footer returns when generation completes", plainFooter.hidden === false);
await sleep(100); // let the message-node markdown throttle timer fire

// Timer cleanup
check("relative-time refresh timer is scheduled", pendingTimers.size > 0);
plainNode.destroy();
check("destroy clears all footer timers", pendingTimers.size === 0);
check("destroy detaches the node", !plainNode.el.isConnected);

// --- Grouped agent run --------------------------------------------------------

const runStart = NOW - 5 * 60_000;
const runMessages = [
  { id: "u1", role: "user", runId: "u1", createdAt: runStart, blocks: [{ id: "ut", type: "text", text: "do it" }] },
  {
    id: "a1",
    role: "assistant",
    runId: "u1",
    createdAt: runStart + 10_000,
    blocks: [
      { id: "r1", type: "reasoning", text: "thinking hard" },
      { id: "tc1", type: "tool_call", name: "read_file", input: {} },
    ],
  },
  {
    id: "a2",
    role: "assistant",
    runId: "u1",
    createdAt: runStart + 100_000,
    updatedAt: runStart + 110_000,
    blocks: [{ id: "t2", type: "text", text: "Final answer" }],
  },
];

const items = buildFeedItems(runMessages, { generatingMessageId: null });
check("run collapses into a single agent-run item", items.length === 1 && items[0].type === "agent_run");
const runItem = items[0];
check("run duration computed", runItem.durationMs === 110_000);

const runNode = createFeedNode(runItem, config);
window.document.body.appendChild(runNode.el);
runNode.update(runItem, makeCtx({ messages: runMessages }));

check("agent run type", runNode.type === "agent_run");
check(
  "agent run gets exactly one footer",
  runNode.el.querySelectorAll(".mur-turn-footer").length === 1,
);
const runFooter = runNode.el.querySelector(".mur-turn-footer");
check("run footer mounts after the final visible message", runNode.el.lastElementChild === runFooter);
check("run footer outside collapsible work segment", !runFooter.closest(".mur-agent-run-work"));

const runTime = runFooter.querySelector("time");
check("run footer relative time", runTime?.textContent === "3m ago");
const runTooltip = runFooter.querySelector(".mur-turn-footer-tooltip");
check(
  "run tooltip shows absolute time",
  runTooltip?.querySelector(".mur-turn-footer-tooltip-time")?.textContent ===
    new Date(runItem.finalMessage.updatedAt).toLocaleString(),
);
check(
  "run tooltip shows Worked for duration",
  runTooltip?.querySelector(".mur-turn-footer-tooltip-duration")?.textContent === "Worked for 1m 50s",
);

// Run copy copies the final message text
runFooter.querySelector('button[aria-label="Copy response"]').click();
await sleep(0);
check("run copy uses the final message text", copiedText === "Final answer");

// Footer persists across work-segment collapse/expand toggles
const collapsedItems = buildFeedItems(runMessages, {
  generatingMessageId: null,
  isWorkSegmentExpanded: () => false,
});
runNode.update(collapsedItems[0], makeCtx({ messages: runMessages }));
check(
  "footer survives work-segment collapse",
  runNode.el.querySelector(".mur-turn-footer") === runFooter && runFooter.isConnected,
);
const expandedItems = buildFeedItems(runMessages, {
  generatingMessageId: null,
  isWorkSegmentExpanded: () => true,
});
runNode.update(expandedItems[0], makeCtx({ messages: runMessages }));
check(
  "footer survives work-segment expansion",
  runNode.el.querySelector(".mur-turn-footer") === runFooter && runFooter.isConnected,
);
check("run footer still the last element", runNode.el.lastElementChild === runFooter);

// Hidden while the run is active
runNode.update(runItem, makeCtx({ messages: runMessages, generatingMessageId: "a2" }));
check("run footer hides while the run is generating", runFooter.hidden === true);
await sleep(100); // let the message-node markdown throttle timer fire

runNode.destroy();
check("run destroy clears footer timers", pendingTimers.size === 0);

if (failures.length > 0) {
  console.error(`turn-footer: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("turn-footer: all assertions passed");
