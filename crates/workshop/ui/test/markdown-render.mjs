// The markdown renderer (src/parts/agent/markdown-render.ts) in jsdom: marked
// output lands under a .ws-markdown-content root with the right elements for
// headings, paragraphs, emphasis, links, lists, blockquotes, tables, and
// images (including the =WxH dimension suffix); fenced code blocks get
// Shiki's theme colors once markdownReady resolves; and model-authored
// attacks - javascript: hrefs, <script> tags, inline event handlers -
// are stripped by the DOMPurify pass inside renderMarkdown. highlightCode
// is exercised directly for its plain-fallback paths. Run:
// node test/markdown-render.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { mock } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { resolver, rulesOf, valueIn } from "./helpers/css-values.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { renderMarkdown, highlightCode, markdownReady, MarkdownStream, FADE_MS } from "./src/parts/agent/markdown-render.ts";
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
const { renderMarkdown, highlightCode, markdownReady, MarkdownStream, FADE_MS } = await import(
  pathToFileURL(bundlePath).href
);

// The clipboard the code blocks' copy buttons write to, recorded.
const clipboard = [];
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {
    clipboard: {
      writeText: async (text) => {
        clipboard.push(text);
      },
    },
  },
});

// Highlighting is the async half of the contract; everything below runs
// after readiness, so code blocks exercise the Shiki path.
await markdownReady;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Renders text into a container element and returns the .ws-markdown-content root.
function render(text, options) {
  const container = document.createElement("div");
  container.append(renderMarkdown(text, options));
  return container.firstElementChild;
}

// --- Structure -----------------------------------------------------------

{
  const root = render("# Title\n\nA paragraph of prose.");
  check("the rendered fragment's root has the ws-markdown-content class",
    root?.classList.contains("ws-markdown-content"));
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
  check("a keyword in the block reads the theme's keyword slot",
    pre?.innerHTML.includes("color:var(--shiki-token-keyword)") === true);
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

// --- The CSS-variables theme -----------------------------------------------

// Cursor colors code through a Shiki theme whose every color is a CSS
// variable (--shiki-foreground, --shiki-token-keyword, ...) that the
// stylesheet fills from the skin's --syntax-* tokens. The markup carries
// only var() references; the stylesheet must declare every slot the theme
// can emit, and each must resolve to a color.
{
  const SLOTS = [
    "foreground",
    "background",
    "token-comment",
    "token-string",
    "token-string-expression",
    "token-constant",
    "token-keyword",
    "token-variable",
    "token-language-variable",
    "token-parameter",
    "token-constant-variable",
    "token-property",
    "token-function",
    "token-type",
    "token-class",
    "token-tag",
    "token-attribute",
    "token-punctuation",
    "token-link",
  ];
  const resolve = await resolver();
  const css = await readFile(path.join(testDir, "..", "src", "parts", "agent", "markdown-render.css"), "utf8");
  const rules = rulesOf(css);
  const slotValue = (slot) => resolve(valueIn(rules, ".ws-markdown-content", `--shiki-${slot}`));

  for (const slot of SLOTS) {
    const value = slotValue(slot);
    check(`--shiki-${slot} is declared and resolves to a color`, typeof value === "string" && /^#[0-9a-f]{6,8}$|^transparent$/.test(value));
  }
  check("the foreground is #F0F0F0", slotValue("foreground") === "#f0f0f0");
  check("comments are #F0F0F099", slotValue("token-comment") === "#f0f0f099");
  check("the skin's --syntax-fg is #F0F0F0", resolve("var(--syntax-fg)") === "#f0f0f0");
  check("the skin's --syntax-comment is #F0F0F099", resolve("var(--syntax-comment)") === "#f0f0f099");
  check("the block background is the skin's #181818", slotValue("background") === "#181818");

  // The theme draws on no slot the stylesheet leaves out, across the kinds of token a block holds.
  const samples = [
    ["typescript", 'import { a } from "b";\n// note\nclass Foo<T> extends Bar {\n  static readonly N = 10;\n  constructor(private p: number) { super(); this.q = `x${p}`; }\n  run(arg: T): void { const re = /ab+c/g; if (this.q === "x") { console.log(arg, true, null, Foo.N); } }\n}\n'],
    ["html", '<div class="a" id="b">text</div>\n'],
    ["css", "a { color: red; margin: 0 }\n"],
    ["json", '{"a": [1, true, null, "x"]}\n'],
    ["python", "@deco\ndef f(x, *, y=1):\n    return [x for x in range(3)]  # c\n"],
    ["markdown", "# Title\n\n[link](http://example.test) and `code` and **bold**\n\n> quoted\n"],
    ["rust", "fn main<'a>(x: &'a str) { let y = Vec::<u8>::new(); println!(\"{x}\"); }\n"],
  ];
  const used = new Set();
  for (const [lang, code] of samples) {
    for (const match of highlightCode(code, lang).matchAll(/var\(--shiki-([a-z-]+)\)/g)) used.add(match[1]);
  }
  check("the samples exercise a spread of slots", used.size >= 8);
  check(
    `every slot the theme emits is declared (${[...used].filter((slot) => !SLOTS.includes(slot)).join(", ") || "none missing"})`,
    [...used].every((slot) => SLOTS.includes(slot)),
  );
  const comment = highlightCode("// hello\nlet x = 1;", "typescript");
  check(
    "a comment is italic through the same slot",
    comment.includes("color:var(--shiki-token-comment);font-style:italic"),
  );
  check(
    "no color in a highlighted block is a literal hex",
    !/(?:color|background-color):#[0-9a-fA-F]{3,8}/.test(highlightCode("fn main() { let x = 1; }", "rust")),
  );
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

// --- The streaming fade -------------------------------------------------------

// A stream rendered at scripted times: the text a render adds past the
// previous render's text fades in, word by word, and only text younger than
// FADE_MS is wrapped, with a negative animation-delay of how far into the
// fade it already is.
{
  const stream = new MarkdownStream();
  const draw = (text, now, streaming = true) => {
    const container = document.createElement("div");
    container.append(stream.render(text, { streaming, now }));
    return container.firstElementChild;
  };
  const fades = (root) => [...root.querySelectorAll(".ws-fade-in")];
  const words = (root) => fades(root).map((span) => span.textContent);
  const delays = (root) => fades(root).map((span) => Number.parseFloat(span.style.animationDelay));

  check("the fade lasts 150ms", FADE_MS === 150);

  let root = draw("Hello", 1000);
  check(
    "the first render of a stream fades all of its text in, with no delay yet",
    words(root).join(",") === "Hello" && delays(root)[0] === 0,
  );

  root = draw("Hello world", 1050);
  check(
    "a delta fades its own word in; the earlier word, 50ms in, carries its fade on",
    words(root).join(",") === "Hello,world" && delays(root)[0] === -50 && delays(root)[1] === 0,
  );
  check("the text itself is unchanged by the wrapping", root.querySelector("p")?.textContent === "Hello world");

  root = draw("Hello world again", 1200);
  check(
    "text older than 150ms is no longer wrapped: only the new word fades",
    words(root).join(",") === "again" && root.querySelector("p")?.textContent === "Hello world again",
  );

  root = draw("Hello world again", 1500);
  check("a render with nothing new wraps nothing once the fade is over", fades(root).length === 0);

  root = draw("Hello world again and more", 1600, false);
  check(
    "a settle that still has new text keeps fading it (a stream that streamed keeps its fade)",
    words(root).join(",") === "and,more",
  );
}

{
  const stream = new MarkdownStream();
  const container = document.createElement("div");
  container.append(stream.render("settled history", { streaming: false, now: 5 }));
  check(
    "a reply that never streamed renders plain, with no fade",
    container.querySelectorAll(".ws-fade-in").length === 0 &&
      container.textContent === "settled history",
  );
}

{
  const stream = new MarkdownStream();
  const container = document.createElement("div");
  container.append(stream.render("intro\n\n```js\nconst a = 1;\n```\n\noutro", { streaming: true, now: 10 }));
  const spans = [...container.querySelectorAll(".ws-fade-in")].map((span) => span.textContent);
  check(
    "prose fades but a code block never does",
    spans.join(",") === "intro,outro" &&
      container.querySelectorAll("pre .ws-fade-in").length === 0 &&
      container.querySelector("pre")?.textContent?.includes("const a = 1;") === true,
  );
  check(
    "the copy button is chrome and never fades",
    container.querySelector(".ws-code-block__copy .ws-fade-in") === null &&
      container.querySelector(".ws-code-block__copy")?.textContent === "Copy code",
  );
}

{
  const stream = new MarkdownStream();
  const container = document.createElement("div");
  container.append(stream.render("one **two", { streaming: true, now: 100 }));
  container.replaceChildren(stream.render("one **two** three", { streaming: true, now: 400 }));
  check(
    "when earlier text changes (markup closing), the text from the change on fades again",
    [...container.querySelectorAll(".ws-fade-in")].map((span) => span.textContent).join(",") === "two,three" &&
      container.querySelector("strong")?.textContent === "two",
  );
}

{
  window.matchMedia = (query) => ({ matches: true, media: query });
  const stream = new MarkdownStream();
  const container = document.createElement("div");
  container.append(stream.render("moving words", { streaming: true, now: 1 }));
  check(
    "under reduced motion nothing is wrapped to fade",
    container.querySelectorAll(".ws-fade-in").length === 0 && container.textContent === "moving words",
  );
  delete window.matchMedia;
}

{
  const stream = new MarkdownStream();
  const container = document.createElement("div");
  container.append(
    stream.render(
      '<script>alert(1)</script>\n\n[x](javascript:alert(1)) <img src="x" onerror="alert(1)">',
      { streaming: true, now: 1 },
    ),
  );
  check(
    "DOMPurify still runs on a stream: no script, no javascript: href, no handler",
    container.querySelector("script") === null &&
      container.querySelector("a")?.getAttribute("href") === null &&
      container.querySelector("[onerror]") === null,
  );
}

// --- No caret -----------------------------------------------------------------

{
  const css = (await readFile(path.join(testDir, "..", "src", "parts", "agent", "markdown-render.css"), "utf8")).toLowerCase();
  const session = (await readFile(path.join(testDir, "..", "src", "parts", "agent", "agent-session.css"), "utf8")).toLowerCase();
  check("no stylesheet draws a streaming caret", !css.includes("caret") && !session.includes("caret"));
  const root = render("partial text", { streaming: true });
  check("a streaming render adds no caret element", root?.querySelector("[class*='caret']") === null);
}

// --- Copy code ---------------------------------------------------------------------

{
  mock.timers.enable({ apis: ["setTimeout"] });
  const root = render("```js\nconst a = 1;\n```");
  const holder = root?.querySelector(".ws-code-block");
  const button = holder?.querySelector(".ws-code-block__copy");
  check(
    "a fenced block sits in a holder with a Copy code button after its pre and no language label",
    holder?.firstElementChild?.tagName === "PRE" &&
      button?.textContent === "Copy code" &&
      button?.previousElementSibling === holder.firstElementChild &&
      holder.querySelector("[class*='lang']") === null,
  );
  button?.click();
  for (let turn = 0; turn < 5; turn++) await Promise.resolve();
  check(
    "clicking copies the block's code and reads Copied",
    clipboard.at(-1)?.includes("const a = 1;") === true &&
      button?.textContent === "Copied" &&
      button.classList.contains("ws-code-block__copy--copied"),
  );
  mock.timers.tick(2000);
  check(
    "the button reads Copy code again after a moment",
    button?.textContent === "Copy code" && !button.classList.contains("ws-code-block__copy--copied"),
  );
  mock.timers.reset();
  check(
    "an unknown language still gets the button",
    render("```nope\nx\n```")?.querySelector(".ws-code-block__copy") !== null,
  );
}

if (failures.length > 0) {
  console.error(`markdown-render: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("markdown-render: all assertions passed");
process.exit(0);
