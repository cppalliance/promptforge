// Unit test for the tool activity block
// (src/chat/plugins/tools/tools-plugin.ts). Bundles the TS module with
// esbuild (CSS import stripped), imports it via a data URL, and drives it
// against jsdom. Covers: the collapsed one-line autoscrolling window (new
// line in, previous line out, constant height), reduced-motion instant
// replacement, the expanded preserved log with status icons and per-row
// detail chevrons, the resting completion summary, three consecutive calls
// folding into one block per run, collapse-never-discards, and the log's
// scroll-pinning disengage/re-engage.
// Run: node test/tool-activity.mjs
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
  entryPoints: [path.join(uiDir, "..", "src", "chat", "plugins", "tools", "tools-plugin.ts")],
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
const { ToolsPlugin } = mod;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function toolCall(id, name, status, args = {}) {
  return { id, type: "tool_call", toolCallId: `tc-${id}`, name, argsText: JSON.stringify(args), status };
}

function toolResult(callId, outputText, isError = false) {
  return { id: `res-${callId}`, type: "tool_result", toolCallId: `tc-${callId}`, outputText, isError };
}

// Renders one engine pass: a fresh messages array per pass, blocks in order,
// isGenerating only on the generating block index. Result blocks live on a
// trailing tool-role message, mirroring the engine's layout.
function renderPass(plugin, containers, assistantMessage, toolMessage, generatingIndex) {
  const messages = toolMessage ? [assistantMessage, toolMessage] : [assistantMessage];
  assistantMessage.blocks.forEach((block, index) => {
    plugin.onBlockRender(block, containers[index], generatingIndex === index, {
      message: assistantMessage,
      messages,
      blockIndex: index,
    });
  });
}

function assistantMsg(id, blocks) {
  return { id, role: "assistant", blocks };
}

function toolMsg(id, blocks) {
  return { id, role: "tool", blocks };
}

function freshContainer(parent) {
  const container = window.document.createElement("div");
  parent.appendChild(container);
  return container;
}

const windowLine = (root) => root.querySelector(".mur-tool-run-window .mur-tool-run-line:last-child");

// --- Collapsed while working: autoscroll at constant height -------------------

const plugin = ToolsPlugin();
const parent = window.document.createElement("div");
const c1 = freshContainer(parent);

renderPass(plugin, [c1], assistantMsg("mA", [toolCall("1", "read_file", "running", { path: "a.ts" })]), null, 0);

check("run block renders into the first container", !!c1.querySelector(".mur-tool-run"));
check("collapsed by default", c1.querySelector(".mur-tool-run-toggle")?.getAttribute("aria-expanded") === "false");
check("log hidden while collapsed", c1.querySelector(".mur-tool-run-log")?.hidden === true);
check("window shows the first activity line", windowLine(c1)?.textContent?.includes("read_file"));
check("host marked running", c1.classList.contains("mur-tool-run-host--running"));

// Second call arrives: the previous line scrolls out, the new one rests.
const c2 = freshContainer(parent);
renderPass(
  plugin,
  [c1, c2],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "running", { pattern: "foo" }),
  ]),
  toolMsg("mT", [toolResult("1", "file body")]),
  1,
);

check("second call folds into the same run block", parent.querySelectorAll(".mur-tool-run").length === 1);
check("member container hides", c2.hidden === true);
check("window line advances to the new activity", windowLine(c1)?.textContent?.includes("grep"));
check(
  "previous line scrolls out while the new one enters",
  c1.querySelectorAll(".mur-tool-run-line").length === 2 &&
    !!c1.querySelector(".mur-tool-run-line--exit") &&
    !!c1.querySelector(".mur-tool-run-line--enter"),
);
await sleep(250);
check("one line rests after the animation", c1.querySelectorAll(".mur-tool-run-line").length === 1);
check("resting line is the newest activity", windowLine(c1)?.textContent?.includes("grep"));

// Constant height is a stylesheet contract: fixed-height window, overflow
// hidden, transform-only line motion.
const css = await readFile(path.join(uiDir, "..", "src", "chat", "plugins", "tools", "tools.css"), "utf8");
const windowRule = /\.mur-tool-run-window\s*\{([^}]*)\}/.exec(css);
check("window rule exists", !!windowRule);
check("window height is fixed", /height:\s*1\.45em/.test(windowRule?.[1] ?? ""));
check("window clips the scrolling lines", /overflow:\s*hidden/.test(windowRule?.[1] ?? ""));
check("line motion is transform-only", /\.mur-tool-run-line--go\s*\{[^}]*transition:\s*transform/.test(css));
check("spinner animates transform", /@keyframes mur-tool-spin[\s\S]*?transform:\s*rotate/.test(css));
check(
  "reduced motion drops the line transition",
  /prefers-reduced-motion:\s*reduce[\s\S]*?mur-tool-run-line--go[\s\S]*?transition:\s*none/.test(css),
);
check(
  "reduced motion stills the spinner",
  /prefers-reduced-motion:\s*reduce[\s\S]*?mur-tool-row-spinner[\s\S]*?animation:\s*none/.test(css),
);
check("done icon uses the green token", /\.mur-tool-row-icon--done\s*\{[^}]*var\(--mur-success/.test(css));
check("error icon uses the danger token", /\.mur-tool-row-icon--error\s*\{[^}]*var\(--mur-danger-text/.test(css));

// --- Reduced motion: instant line replacement ----------------------------------

window.matchMedia = () => ({
  matches: true,
  media: "(prefers-reduced-motion: reduce)",
  onchange: null,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
  dispatchEvent: () => false,
});

const rmPlugin = ToolsPlugin();
const rmParent = window.document.createElement("div");
const rmC1 = freshContainer(rmParent);
renderPass(rmPlugin, [rmC1], assistantMsg("rmA", [toolCall("1", "read_file", "running", { path: "a.ts" })]), null, 0);
const rmC2 = freshContainer(rmParent);
renderPass(
  rmPlugin,
  [rmC1, rmC2],
  assistantMsg("rmA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "running", { pattern: "foo" }),
  ]),
  toolMsg("rmT", [toolResult("1", "file body")]),
  1,
);
check("reduced motion replaces the line instantly", rmC1.querySelectorAll(".mur-tool-run-line").length === 1);
check("reduced motion shows the newest line", windowLine(rmC1)?.textContent?.includes("grep"));
check("no animation classes under reduced motion", !rmC1.querySelector(".mur-tool-run-line--enter"));

delete window.matchMedia;

// --- Three consecutive calls fold into one block per run ------------------------

const c3 = freshContainer(parent);
renderPass(
  plugin,
  [c1, c2, c3],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "complete", { pattern: "foo" }),
    toolCall("3", "write_file", "running", { path: "b.ts" }),
  ]),
  toolMsg("mT", [toolResult("1", "file body"), toolResult("2", "3 matches")]),
  2,
);

check("three calls still form exactly one run block", parent.querySelectorAll(".mur-tool-run").length === 1);
check("third member container hides", c3.hidden === true);
check("leader container stays visible", c1.hidden === false);
check("log preserves all three rows", c1.querySelectorAll(".mur-tool-run-log .mur-tool-row").length === 3);

// --- Completion: the summary line rests -----------------------------------------

renderPass(
  plugin,
  [c1, c2, c3],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "complete", { pattern: "foo" }),
    toolCall("3", "write_file", "complete", { path: "b.ts" }),
  ]),
  toolMsg("mT", [toolResult("1", "file body"), toolResult("2", "3 matches"), toolResult("3", "wrote 10 lines")]),
  -1,
);

check("completion rests the summary line", windowLine(c1)?.textContent === "3 actions completed");
check("host marked complete", c1.classList.contains("mur-tool-run-host--complete"));
check("summary announced in the live region", c1.querySelector(".mur-tool-run-sr-only")?.textContent === "3 actions completed");

renderPass(
  plugin,
  [c1, c2, c3],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "complete", { pattern: "foo" }),
    toolCall("3", "write_file", "complete", { path: "b.ts" }),
  ]),
  toolMsg("mT", [toolResult("1", "file body"), toolResult("2", "3 matches"), toolResult("3", "wrote 10 lines")]),
  -1,
);
await sleep(250);
check("the resting summary does not repeat", c1.querySelectorAll(".mur-tool-run-line").length === 1);

// --- Expanded: the full preserved log ---------------------------------------------

c1.querySelector(".mur-tool-run-toggle").click();
const log = c1.querySelector(".mur-tool-run-log");
check("expanding reveals the log", log.hidden === false);
check("toggle aria-expanded flips", c1.querySelector(".mur-tool-run-toggle")?.getAttribute("aria-expanded") === "true");
const rows = log.querySelectorAll(".mur-tool-row");
check("all three rows present", rows.length === 3);
check("done row shows the green check", rows[0].querySelector(".mur-tool-row-icon--done")?.textContent === "✓");
check("row carries a one-line summary", rows[1].textContent.includes("grep"));
check("every row has its own chevron toggle", rows[2].querySelector(".mur-tool-row-toggle") !== null);

// Per-row chevron unfolds arguments and result.
const rowToggle = rows[0].querySelector(".mur-tool-row-toggle");
rowToggle.click();
const details = rows[0].querySelector(".mur-tool-row-details");
check("row chevron expands the details", details.hidden === false);
check("row aria-expanded flips", rowToggle.getAttribute("aria-expanded") === "true");
check("arguments rendered", details.querySelector(".mur-tool-pre")?.textContent?.includes('"a.ts"'));
check("result rendered", details.textContent.includes("file body"));
rowToggle.click();
check("row chevron collapses the details", details.hidden === true);

// Collapsing the run hides history without discarding it.
c1.querySelector(".mur-tool-run-toggle").click();
check("collapsing hides the log", log.hidden === true);
check("history is preserved while hidden", log.querySelectorAll(".mur-tool-row").length === 3);
c1.querySelector(".mur-tool-run-toggle").click();
check("re-expanding restores the same rows", log.querySelectorAll(".mur-tool-row").length === 3);

// --- Log scroll-pinning: disengage on scroll-up, re-engage at bottom -------------

let storedScrollTop = 0;
Object.defineProperty(log, "scrollHeight", { configurable: true, get: () => 1000 });
Object.defineProperty(log, "clientHeight", { configurable: true, get: () => 100 });
Object.defineProperty(log, "scrollTop", {
  configurable: true,
  get: () => storedScrollTop,
  set: (value) => {
    storedScrollTop = value;
  },
});

const c4 = freshContainer(parent);
renderPass(
  plugin,
  [c1, c2, c3, c4],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "complete", { pattern: "foo" }),
    toolCall("3", "write_file", "complete", { path: "b.ts" }),
    toolCall("4", "read_file", "running", { path: "c.ts" }),
  ]),
  toolMsg("mT", [toolResult("1", "file body"), toolResult("2", "3 matches"), toolResult("3", "wrote 10 lines")]),
  3,
);
check("new row while expanded pins to the bottom", storedScrollTop === 1000);

storedScrollTop = 400;
log.dispatchEvent(new window.Event("scroll"));
renderPass(
  plugin,
  [c1, c2, c3, c4],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "complete", { pattern: "foo" }),
    toolCall("3", "write_file", "complete", { path: "b.ts" }),
    toolCall("4", "read_file", "running", { path: "c.ts" }),
  ]),
  toolMsg("mT", [toolResult("1", "file body"), toolResult("2", "3 matches"), toolResult("3", "wrote 10 lines")]),
  3,
);
check("scroll-up disengages auto-pin", storedScrollTop === 400);

storedScrollTop = 900; // scrollHeight - clientHeight: back at the bottom
log.dispatchEvent(new window.Event("scroll"));
renderPass(
  plugin,
  [c1, c2, c3, c4],
  assistantMsg("mA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "grep", "complete", { pattern: "foo" }),
    toolCall("3", "write_file", "complete", { path: "b.ts" }),
    toolCall("4", "read_file", "complete", { path: "c.ts" }),
  ]),
  toolMsg("mT", [
    toolResult("1", "file body"),
    toolResult("2", "3 matches"),
    toolResult("3", "wrote 10 lines"),
    toolResult("4", "c body"),
  ]),
  -1,
);
check("scrolling back to the bottom re-engages auto-pin", storedScrollTop === 1000);
check("run of four rests a new summary", windowLine(c1)?.textContent === "4 actions completed");

// --- Error status ------------------------------------------------------------------

const errPlugin = ToolsPlugin();
const errParent = window.document.createElement("div");
const e1 = freshContainer(errParent);
const e2 = freshContainer(errParent);
renderPass(
  errPlugin,
  [e1, e2],
  assistantMsg("eA", [
    toolCall("1", "read_file", "complete", { path: "a.ts" }),
    toolCall("2", "write_file", "error", { path: "b.ts" }),
  ]),
  toolMsg("eT", [toolResult("1", "file body"), toolResult("2", "permission denied", true)]),
  -1,
);

check("error run rests a mixed summary", windowLine(e1)?.textContent === "1 action completed, 1 failed");
check("host marked error", e1.classList.contains("mur-tool-run-host--error"));
e1.querySelector(".mur-tool-run-toggle").click();
const errRows = e1.querySelectorAll(".mur-tool-run-log .mur-tool-row");
check("error row shows the red X", errRows[1].querySelector(".mur-tool-row-icon--error")?.textContent === "×");
errRows[1].querySelector(".mur-tool-row-toggle").click();
check("error details title reads Error", errRows[1].textContent.includes("Error"));
check("error output rendered", errRows[1].textContent.includes("permission denied"));

// --- Spinner while running ----------------------------------------------------------

const spinPlugin = ToolsPlugin();
const spinParent = window.document.createElement("div");
const s1 = freshContainer(spinParent);
renderPass(spinPlugin, [s1], assistantMsg("sA", [toolCall("1", "read_file", "running", { path: "a.ts" })]), null, 0);
spinParent.querySelector(".mur-tool-run-toggle")?.click();
check("running row shows a spinner", !!spinParent.querySelector(".mur-tool-row-icon--working .mur-tool-row-spinner"));

if (failures.length > 0) {
  console.error(`tool-activity: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("tool-activity: all assertions passed");
