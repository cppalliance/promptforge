// The transcript's tool line (src/parts/agent/transcript/tool-line.ts) and its
// web hover card (web-hover-card.ts) in jsdom, on fake timers. A search or
// fetch line is not expandable; its card opens 600ms after the pointer
// settles on it, never while the call loads, lists the result titles and
// URLs parsed from the search result `{query, results: [...]}`, turns only
// http(s) URLs into links (a `javascript:` URL stays plain text), closes on
// leave, Escape, or a link click, and is absent when the result doesn't
// parse as that shape. A generic tool expands into one block - its
// arguments as indented JSON, a blank line, then its result - and keeps
// its open state across updates. While a call loads only the verb
// shimmers.
// Run: node test/tool-line.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { mock } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { ToolLine, toolBlockText } from "./src/parts/agent/transcript/tool-line.ts";
      export { webCardEntries, WEB_CARD_OPEN_DELAY_MS } from "./src/parts/agent/transcript/web-hover-card.ts";
      export { toolKind, toolLabel } from "./src/parts/agent/transcript/tool-labels.ts";
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
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;

const bundlePath = path.join(os.tmpdir(), "promptforge-tool-line-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { ToolLine, toolBlockText, webCardEntries, WEB_CARD_OPEN_DELAY_MS, toolKind, toolLabel } = await import(
  pathToFileURL(bundlePath).href
);

mock.timers.enable({ apis: ["setTimeout"] });

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** A tool step the way the model builds one. */
function step({ name, tool, args = {}, result = null, loading = false }) {
  const call = { id: "c1", name, tool, args: args === "" ? "" : JSON.stringify(args) };
  return {
    step: "tool",
    key: "c:0:c1",
    call,
    toolKind: toolKind(tool),
    result,
    loading,
    label: toolLabel(call, loading),
  };
}

const searchResult = JSON.stringify({
  query: "q",
  results: [
    { title: "Alpha <b>docs</b>", url: "https://alpha.test/docs", description: "d" },
    { title: "Beta", url: "javascript:alert(1)", description: "" },
    { title: "Gamma", url: "file:///etc/passwd", description: "" },
    { url: "http://delta.test/" },
  ],
});

function mount(initial) {
  const line = new ToolLine();
  document.body.appendChild(line.element);
  line.update(initial);
  const header = line.element.querySelector(".ws-collapsible__header");
  const hover = () => header.dispatchEvent(new window.MouseEvent("mouseenter"));
  const leave = () => header.dispatchEvent(new window.MouseEvent("mouseleave"));
  const card = () => document.querySelector(".ws-web-card");
  const dispose = () => {
    line.dispose();
    line.element.remove();
    card()?.remove();
  };
  return { line, header, hover, leave, card, dispose };
}

// --- The card parses the result ----------------------------------------------------------

{
  check(
    "a search result lists a title and URL per result, a missing title falling back to the URL",
    JSON.stringify(webCardEntries("search", '{"query":"q"}', searchResult)) ===
      JSON.stringify([
        { title: "Alpha <b>docs</b>", url: "https://alpha.test/docs" },
        { title: "Beta", url: "javascript:alert(1)" },
        { title: "Gamma", url: "file:///etc/passwd" },
        { title: "http://delta.test/", url: "http://delta.test/" },
      ]),
  );
  check("a search with no result yet has no card", webCardEntries("search", "{}", null) === null);
  check("a result that isn't JSON has no card", webCardEntries("search", "{}", "plain text") === null);
  check("a result without a results array has no card", webCardEntries("search", "{}", '{"query":"q"}') === null);
  check("an empty results array has no card", webCardEntries("search", "{}", '{"results":[]}') === null);
  check(
    "results that carry no URL are skipped",
    webCardEntries("search", "{}", '{"results":[{"title":"x"},{"title":"y","url":"https://y.test/"}]}')?.length === 1,
  );
  check(
    "a fetch lists the URL it was asked for",
    JSON.stringify(webCardEntries("fetch", '{"url":"https://page.test/"}', null)) ===
      JSON.stringify([{ title: "https://page.test/", url: "https://page.test/" }]),
  );
  check("a fetch with no url argument has no card", webCardEntries("fetch", "{}", null) === null);
}

// --- Hover: 600ms, enabled only when settled, closing -----------------------------------------

{
  const search = (extra) => step({ name: "search", tool: "web/search", args: { query: "q" }, ...extra });
  const { line, header, hover, leave, card, dispose } = mount(search({ result: searchResult }));

  check("a search line isn't expandable: no role, no chevron", header.getAttribute("role") === null && line.element.dataset.expandable === "false");
  check("a search line reads Searched web with its query", header.textContent.includes("Searched web") && header.textContent.includes("q"));

  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS - 1);
  check("the card is not open before 600ms", card() === null);
  mock.timers.tick(1);
  check("the card opens at 600ms", card() !== null);
  const entries = [...card().querySelectorAll(".ws-web-card__entry")];
  check("the card lists every result", entries.length === 4);
  const links = [...card().querySelectorAll("a")];
  check(
    "only http and https URLs become links",
    links.length === 2 &&
      links[0].getAttribute("href") === "https://alpha.test/docs" &&
      links[1].getAttribute("href") === "http://delta.test/",
  );
  check(
    "a javascript: or file: URL stays plain, unclickable text",
    document.querySelector('a[href^="javascript"]') === null &&
      entries[1].querySelector("span.ws-web-card__title")?.textContent === "Beta" &&
      entries[1].querySelector(".ws-web-card__url")?.textContent === "javascript:alert(1)" &&
      entries[2].querySelector("a") === null,
  );
  check(
    "titles are text, never markup",
    card().querySelector("b") === null && links[0].textContent === "Alpha <b>docs</b>",
  );
  check("a link is not draggable", links[0].draggable === false);

  const opened = [];
  links[0].addEventListener("click", (event) => {
    event.preventDefault();
    opened.push(event.target.href);
  });
  links[0].click();
  check("clicking a link opens it and closes the card", opened.length === 1 && card() === null);

  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  leave();
  check("the card survives the moment after the pointer leaves the line", card() !== null);
  mock.timers.tick(200);
  check("the card closes once the pointer has left", card() === null);

  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  leave();
  card().dispatchEvent(new window.MouseEvent("mouseenter"));
  mock.timers.tick(500);
  check("reaching the card keeps it open", card() !== null);
  card().dispatchEvent(new window.MouseEvent("mouseleave"));
  mock.timers.tick(200);
  check("leaving the card closes it", card() === null);

  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape" }));
  check("Escape closes the card", card() === null);

  hover();
  leave();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  check("leaving before 600ms cancels the open", card() === null);
  dispose();
}

{
  const search = (extra) => step({ name: "search", tool: "web/search", args: { query: "q" }, ...extra });
  const { line, hover, card, dispose } = mount(search({ loading: true }));
  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS * 2);
  check("the card is disabled while the call loads", card() === null);
  line.update(search({ result: searchResult }));
  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  check("the same line opens its card once the call finishes", card() !== null);
  line.update(search({ loading: true, result: searchResult }));
  check("a call that starts loading closes an open card", card() === null);
  dispose();
}

{
  const { hover, card, header, dispose } = mount(
    step({ name: "search", tool: "web/search", args: { query: "q" }, result: "no results found" }),
  );
  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  check(
    "a result that doesn't parse as search results gives no card, and the line stays plain",
    card() === null && header.getAttribute("role") === null && header.textContent.includes("Searched web"),
  );
  dispose();
}

{
  const { hover, card, dispose } = mount(
    step({ name: "fetch", tool: "web/fetch", args: { url: "https://page.test/a" }, result: "<html>" }),
  );
  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  check(
    "a fetch's card lists its URL as a link",
    card()?.querySelectorAll("a").length === 1 && card().querySelector("a").getAttribute("href") === "https://page.test/a",
  );
  dispose();
}

{
  const { hover, card, dispose } = mount(
    step({ name: "fetch", tool: "web/fetch", args: { url: "javascript:alert(1)" }, result: "x" }),
  );
  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  check(
    "a fetch of a non-http URL lists it as plain text",
    card() !== null && card().querySelector("a") === null && card().textContent.includes("javascript:alert(1)"),
  );
  dispose();
}

// --- The generic block ---------------------------------------------------------------------

{
  const generic = (extra) => step({ name: "read", tool: "fs/read", args: { path: "a" }, ...extra });
  const { line, header, dispose } = mount(generic({}));
  check(
    "a generic tool with arguments is expandable",
    header.getAttribute("role") === "button" && header.getAttribute("aria-expanded") === "false",
  );
  check(
    "its block holds the arguments as indented JSON",
    line.element.querySelector(".ws-tool-block__pre").textContent === '{\n  "path": "a"\n}',
  );
  header.click();
  line.update(generic({ result: "contents <i>here</i>" }));
  check(
    "a result lands after the arguments and a blank line, in an open block that stayed open",
    line.element.dataset.open === "true" &&
      line.element.querySelector(".ws-tool-block__pre").textContent === '{\n  "path": "a"\n}\n\ncontents <i>here</i>' &&
      line.element.querySelector("i") === null,
  );
  header.click();
  line.update(generic({ result: "contents <i>here</i>", loading: false }));
  check("a block the operator closed stays closed across an update", line.element.dataset.open === "false");
  dispose();
}

{
  const bare = step({ name: "ping", tool: "net/ping", args: "" });
  const { header, dispose } = mount(bare);
  check("a generic tool with no arguments and no result has nothing to open", header.getAttribute("role") === null);
  check("a block with neither arguments nor result is empty text", toolBlockText(bare) === "");
  dispose();
}

{
  const raw = step({ name: "odd", tool: "x/odd", args: "" });
  raw.call = { ...raw.call, args: "not json" };
  check("arguments that don't parse show as given", toolBlockText({ ...raw, result: "r" }) === "not json\n\nr");
  check("a result alone is the whole block", toolBlockText({ ...step({ name: "n", tool: "x/n", args: "" }), result: "only" }) === "only");
}

// --- The verb shimmers, nothing else ----------------------------------------------------------

{
  const generic = (loading) => step({ name: "read", tool: "fs/read", args: { path: "a" }, loading });
  const { line, dispose } = mount(generic(true));
  const action = line.element.querySelector(".ws-collapsible__action");
  const details = line.element.querySelector(".ws-collapsible__details");
  const call = line.element.querySelector(".ws-collapsible__call");
  check(
    "while loading the verb shimmers in the tool tone and nothing else does",
    action.classList.contains("ws-shimmer-text") &&
      action.classList.contains("ws-shimmer-text--tool") &&
      !details.classList.contains("ws-shimmer-text") &&
      !call.classList.contains("ws-shimmer-text") &&
      line.element.dataset.loading === "true",
  );
  line.update(generic(false));
  check(
    "the shimmer stops when the call finishes",
    !action.classList.contains("ws-shimmer-text") && line.element.dataset.loading === "false",
  );
  check("the line has no icon, dot, or badge", line.element.querySelectorAll("img, .ws-badge, .ws-dot").length === 0);
  dispose();
}

// --- Ask lines --------------------------------------------------------------------------------

{
  const ask = (loading) => step({ name: "ask", tool: "user-input/ask", args: { question: "?" }, loading });
  const { line, header, hover, card, dispose } = mount(ask(true));
  check(
    "an ask line reads Asking questions with no details",
    header.textContent.startsWith("Asking questions") && line.element.dataset.toolKind === "ask",
  );
  line.update(ask(false));
  hover();
  mock.timers.tick(WEB_CARD_OPEN_DELAY_MS);
  check(
    "an ask line reads Asked questions, isn't expandable, and opens no card",
    header.textContent.startsWith("Asked questions") && header.getAttribute("role") === null && card() === null,
  );
  dispose();
}

if (failures.length > 0) {
  console.error(`tool-line: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("tool-line: all assertions passed");
process.exit(0);
