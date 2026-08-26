// Unit test for the block-memoized streaming markdown renderer
// (src/chat/markdown-blocks.ts). Bundles the TS module with esbuild,
// imports it via a data URL, and drives it against jsdom. Covers:
// unterminated-construct repair (code fence, bold, link, table) mid-stream,
// the renderSafeHTML sanitizer as final pass, and parse-count
// instrumentation proving completed blocks are never re-parsed.
// Run: node test/markdown-blocks.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
globalThis.document = dom.window.document;
globalThis.DOMParser = dom.window.DOMParser;
globalThis.NodeFilter = dom.window.NodeFilter;
globalThis.Node = dom.window.Node;
globalThis.HTMLElement = dom.window.HTMLElement;

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "chat", "markdown-blocks.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const code = bundle.outputFiles[0].text;
const mod = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
const { splitMarkdownBlocks, repairStreamingMarkdown, StreamingMarkdownRenderer } = mod;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Block splitting -------------------------------------------------------

const splitBasic = splitMarkdownBlocks("alpha\n\nbeta\ngamma");
check("split yields two blocks", splitBasic.length === 2);
check("first block complete", splitBasic[0]?.complete === true && splitBasic[0]?.text === "alpha");
check("last block is the tail", splitBasic[1]?.complete === false && splitBasic[1]?.text === "beta\ngamma");

const splitFence = splitMarkdownBlocks("```\na\n\nb\n```\n\nc");
check("blank line inside a fence does not split", splitFence.length === 2);
check("fenced block kept whole", splitFence[0]?.text === "```\na\n\nb\n```");

const splitBoundary = splitMarkdownBlocks("a\n\n");
check("text ending on a blank line has no tail", splitBoundary.length === 1 && splitBoundary[0]?.complete === true);

// --- Tail repair -----------------------------------------------------------

check("unclosed fence is closed", repairStreamingMarkdown("```js\nconst x = 1;") === "```js\nconst x = 1;\n```\n");
check("open bold is closed", repairStreamingMarkdown("intro **bold words") === "intro **bold words**");
check("open italic is closed", repairStreamingMarkdown("intro *it") === "intro *it*");
check("escaped star is literal", repairStreamingMarkdown("a \\* b") === "a \\* b");
check("incomplete link is closed", repairStreamingMarkdown("see [docs](https://example.com/pa") === "see [docs](https://example.com/pa)");
check("complete link untouched", repairStreamingMarkdown("see [docs](https://example.com/)") === "see [docs](https://example.com/)");
check("partial table delimiter completed", repairStreamingMarkdown("| A | B |\n| --") === "| A | B |\n| --- | --- |");
check("plain text untouched", repairStreamingMarkdown("just words") === "just words");

// --- Rendered output: healed mid-stream constructs --------------------------

const fenceEl = document.createElement("div");
const fenceRenderer = new StreamingMarkdownRenderer(fenceEl);
await fenceRenderer.render("```js\nconst x = 1;", false);
check("unclosed fence mid-stream renders pre>code", !!fenceEl.querySelector("pre code"));
check("fenced code content rendered", fenceEl.textContent.includes("const x = 1;"));

const boldEl = document.createElement("div");
const boldRenderer = new StreamingMarkdownRenderer(boldEl);
await boldRenderer.render("intro **bold words", false);
const strong = boldEl.querySelector("strong");
check("open bold mid-stream renders <strong>", !!strong && strong.textContent.includes("bold words"));

const linkEl = document.createElement("div");
const linkRenderer = new StreamingMarkdownRenderer(linkEl);
await linkRenderer.render("see [docs](https://example.com/pa", false);
const anchor = linkEl.querySelector('a[href="https://example.com/pa"]');
check("incomplete link mid-stream renders as anchor", !!anchor);
check("sanitizer stamped target/rel on healed anchor", anchor?.getAttribute("target") === "_blank" && anchor?.getAttribute("rel") === "noopener");

const tableEl = document.createElement("div");
const tableRenderer = new StreamingMarkdownRenderer(tableEl);
await tableRenderer.render("| A | B |\n| --", false);
check("partial table mid-stream renders a table", !!tableEl.querySelector("table"));

// --- Sanitizer is the final pass on every block -----------------------------

const xssEl = document.createElement("div");
const xssRenderer = new StreamingMarkdownRenderer(xssEl);
await xssRenderer.render("hello <script>alert(1)</script>", false);
check("script tag is escaped by the sanitizer", !xssEl.querySelector("script"));
check("escaped markup stays visible as text", xssEl.textContent.includes("alert(1)"));

// --- Memoization: completed blocks are not re-parsed ------------------------

const memoEl = document.createElement("div");
const memo = new StreamingMarkdownRenderer(memoEl);
await memo.render("para one\n\npara two", false);
check("initial render parses each block once", memo.parseCount === 2);
await memo.render("para one\n\npara two", false);
check("identical text re-parses nothing", memo.parseCount === 2);
await memo.render("para one\n\npara two grows", false);
check("only the tail block is re-parsed", memo.parseCount === 3);
await memo.render("para one\n\npara two grows\n\nthird", false);
check("a completed block whose source is unchanged is not re-parsed", memo.parseCount === 4);
await memo.render("para one\n\npara two grows\n\nthird", true);
check("finalize re-parses nothing when sources are stable", memo.parseCount === 4);
check("all three paragraphs rendered", memoEl.querySelectorAll("p").length === 3);

const healTailEl = document.createElement("div");
const healTail = new StreamingMarkdownRenderer(healTailEl);
await healTail.render("open **bo", false);
check("healed tail parsed once", healTail.parseCount === 1);
await healTail.render("open **bo", true);
check("finalize re-parses a tail that was rendered healed", healTail.parseCount === 2);

if (failures.length > 0) {
  console.error(`markdown-blocks: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("markdown-blocks: all assertions passed");
