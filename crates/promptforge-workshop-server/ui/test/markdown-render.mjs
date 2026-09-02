// The markdown renderer (src/ui/markdown-render.ts) in jsdom: marked
// output lands under a .markdown-content root with the right elements for
// headings, paragraphs, emphasis, links, lists, blockquotes, tables, and
// images (including the =WxH dimension suffix); fenced code blocks carry
// Shiki's theme colors once markdownReady resolves; and model-authored
// attacks - javascript: hrefs, <script> tags, inline event handlers -
// are stripped by the DOMPurify pass inside renderMarkdown. highlightCode
// is exercised directly for its plain-fallback paths. Run:
// node test/markdown-render.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { renderMarkdown, highlightCode, markdownReady } from "./src/ui/markdown-render.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module under test imports its colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

// DOMPurify reads the global window at module load, so the jsdom globals
// must exist before the bundle is imported.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;

const bundlePath = path.join(os.tmpdir(), "promptforge-markdown-render-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { renderMarkdown, highlightCode, markdownReady } = await import(
  pathToFileURL(bundlePath).href
);

// Highlighting is the async half of the contract; everything below runs
// after readiness, so code blocks exercise the Shiki path.
await markdownReady;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Renders text into a host element and returns the .markdown-content root.
function render(text, options) {
  const host = document.createElement("div");
  host.append(renderMarkdown(text, options));
  return host.firstElementChild;
}

// --- Structure -----------------------------------------------------------

{
  const root = render("# Title\n\nA paragraph of prose.");
  check("the rendered fragment's root carries the markdown-content class",
    root?.classList.contains("markdown-content"));
  check("a level-1 heading renders as an h1 with its text",
    root?.querySelector("h1")?.textContent === "Title");
  check("a paragraph renders as a p with its text",
    root?.querySelector("p")?.textContent === "A paragraph of prose.");
}

{
  const root = render("Some **bold** and *italic* and `inline code`.");
  check("double asterisks render as strong",
    root?.querySelector("strong")?.textContent === "bold");
  check("single asterisks render as em",
    root?.querySelector("em")?.textContent === "italic");
  check("backticks render as inline code",
    root?.querySelector("p code")?.textContent === "inline code");
}

{
  const root = render("- one\n- two\n- three");
  const items = root?.querySelectorAll("ul li") ?? [];
  check("a dash list renders as a ul with one li per item",
    root?.querySelector("ul") !== null && items.length === 3);
}

{
  const root = render("1. one\n2. two");
  check("a numbered list renders as an ol",
    root?.querySelectorAll("ol li").length === 2);
}

{
  const root = render("> quoted words");
  check("an angle bracket quote renders as a blockquote",
    root?.querySelector("blockquote")?.textContent?.includes("quoted words") === true);
}

{
  const root = render("| name | value |\n| ---- | ----- |\n| a | 1 |");
  check("a pipe table renders as a table with a header cell",
    root?.querySelector("table th")?.textContent === "name");
  check("a pipe table renders its body cells",
    root?.querySelector("table td")?.textContent === "a");
}

// --- Links and images ----------------------------------------------------

{
  const root = render("[label](https://example.test)");
  const anchor = root?.querySelector("a");
  check("a markdown link keeps its href",
    anchor?.getAttribute("href") === "https://example.test");
  check("a link without a title falls back to the href as its title",
    anchor?.getAttribute("title") === "https://example.test");
  check("a rendered link is not draggable",
    anchor?.getAttribute("draggable") === "false");
}

{
  const root = render('[label](https://example.test "Custom title")');
  check("a link with a title keeps the given title",
    root?.querySelector("a")?.getAttribute("title") === "Custom title");
}

{
  const root = render("![alt text](<image.png =100x200>)");
  const img = root?.querySelector("img");
  check("an image with a dimension suffix drops the suffix from its src",
    img?.getAttribute("src") === "image.png");
  check("an image with a dimension suffix gains width and height attributes",
    img?.getAttribute("width") === "100" && img?.getAttribute("height") === "200");
  check("an image keeps its alt text",
    img?.getAttribute("alt") === "alt text");
}

{
  const root = render("![alt](image.png)");
  const img = root?.querySelector("img");
  check("an image without a dimension suffix keeps its src and gains no dimensions",
    img?.getAttribute("src") === "image.png" && img?.getAttribute("width") === null);
}

{
  const root = render("![alt](<image.png =100x>)");
  const img = root?.querySelector("img");
  check("an image with a width-only dimension suffix gains a width and no height",
    img?.getAttribute("width") === "100" && img?.getAttribute("height") === null);
}

// --- Code blocks ---------------------------------------------------------

{
  const root = render("```rust\nfn main() { let x = 1; }\n```");
  const pre = root?.querySelector("pre");
  check("a fenced code block renders through Shiki once ready",
    pre?.classList.contains("shiki") === true);
  check("a keyword in the block carries the theme's keyword color",
    pre?.innerHTML.includes("color:#82D2CE") === true);
  check("the block's text content survives highlighting",
    pre?.textContent?.includes("fn main()") === true);
}

{
  const root = render("```html\n<script>alert(1)</script>\n```");
  check("markup inside a code block is escaped text, not live elements",
    root?.querySelector("script") === null
      && root?.querySelector("pre")?.textContent?.includes("<script>") === true);
}

{
  const root = render("```not-a-real-language\nsome code\n```");
  const code = root?.querySelector("pre code");
  check("an unknown language degrades to a plain pre with the language class",
    code?.classList.contains("language-not-a-real-language") === true
      && root?.querySelector("pre")?.classList.contains("shiki") === false);
  check("an unknown language keeps its code text",
    code?.textContent?.includes("some code") === true);
}

// --- highlightCode -------------------------------------------------------

{
  const html = highlightCode('{"a": 1}', "json");
  check("highlightCode returns Shiki markup for a loaded language",
    html.includes('class="shiki') && html.includes("style="));
}

{
  const html = highlightCode("plain <text>", "not-a-real-language");
  check("highlightCode escapes and passes through an unknown language unhighlighted",
    html === '<pre><code class="language-not-a-real-language">plain &lt;text&gt;</code></pre>');
}

{
  const html = highlightCode("plain", "");
  check("highlightCode with no language emits a classless plain block",
    html === "<pre><code>plain</code></pre>");
}

// --- Sanitization --------------------------------------------------------

{
  const root = render("[click](javascript:alert(1))");
  const anchor = root?.querySelector("a");
  check("a javascript: href is stripped from a rendered link",
    anchor !== null && anchor.getAttribute("href") === null);
}

{
  const root = render("<script>alert(1)</script>\n\nafter");
  check("a raw script tag in model-authored input never reaches the DOM",
    root?.querySelector("script") === null);
}

{
  const root = render('<img src="x" onerror="alert(1)">\n\n<div onclick="alert(1)">d</div>');
  check("inline event handlers are stripped from rendered markup",
    root?.querySelector("[onerror]") === null && root?.querySelector("[onclick]") === null);
}

// --- Streaming ------------------------------------------------------------

{
  const partial = render("# Streaming\n\npartial **bo", { streaming: true });
  check("streaming mode renders a partial buffer through the same pipeline",
    partial?.querySelector("h1")?.textContent === "Streaming");
}

{
  const section =
    "Some **bold** and *italic* prose with a [link](https://example.test) and `inline code`.\n\n" +
    "```rust\nfn main() { let x = 1; println!(\"{x}\"); }\n```\n\n";
  let doc = "";
  while (doc.length < 5000) doc += section;
  const deltas = 30;
  const start = performance.now();
  for (let i = 1; i <= deltas; i++) {
    renderMarkdown(doc.slice(0, Math.floor((doc.length * i) / deltas)), { streaming: true });
  }
  const elapsed = performance.now() - start;
  check(
    `a full re-parse per delta stays cheap at chat scale (30 deltas of a ${doc.length}-char buffer in ${Math.round(elapsed)}ms)`,
    elapsed < 5000,
  );
}

if (failures.length > 0) {
  console.error(`markdown-render: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("markdown-render: all assertions passed");
process.exit(0);
