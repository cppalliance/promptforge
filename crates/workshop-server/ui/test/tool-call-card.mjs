// The tool-call card (src/ui/tool-call-card.ts) in jsdom: a
// <details>/<summary> card whose header carries the batch's tool name
// with a call-count badge and a status indicator, whose body shows each
// call's arguments as Shiki-highlighted JSON (through Step 2's
// highlightCode) and the matched tool result as a scrollable <pre> fed
// by textContent, and which auto-opens while its batch is running and
// auto-collapses when running flips false. Run:
// node test/tool-call-card.mjs
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
      export { ToolCallCard } from "./src/ui/tool-call-card.ts";
      export { markdownReady } from "./src/ui/markdown-render.ts";
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
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

// DOMPurify (pulled in through markdown-render) reads the global window
// at module load, so the jsdom globals must exist before the import.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;

const bundlePath = path.join(os.tmpdir(), "promptforge-tool-call-card-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { ToolCallCard, markdownReady } = await import(pathToFileURL(bundlePath).href);

// Args highlighting is the async half of the contract; everything below
// runs after readiness so the JSON blocks exercise the Shiki path.
await markdownReady;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// A two-call batch of one tool, in the ToolCallItem shape the service
// emits (services/agent-session.ts).
function makeItem(overrides = {}) {
  return {
    kind: "tool-call",
    calls: [
      { id: "call-1", name: "read_file", args: '{"path":"src/main.ts"}' },
      { id: "call-2", name: "read_file", args: '{"path":"README.md"}' },
    ],
    text: '[{"id":"call-1","name":"read_file","arguments":{"path":"src/main.ts"}}]',
    model: null,
    ...overrides,
  };
}

// --- Header ------------------------------------------------------------------

{
  const card = new ToolCallCard(makeItem(), { running: false });
  const summary = card.element.querySelector(".tool-call-card__summary");
  check(
    "the card is a details element whose first child is its summary",
    card.element.tagName === "DETAILS" && card.element.firstElementChild === summary,
  );
  check(
    "the header shows the batch's shared tool name",
    summary?.querySelector(".tool-call-card__name")?.textContent === "read_file",
  );
  check(
    "the header badge shows the call count",
    summary?.querySelector(".tool-call-card__count")?.textContent === "2",
  );
  check(
    "the header carries a status indicator with a text state for assistive tech",
    summary?.querySelector(".tool-call-card__status") !== null &&
      summary?.querySelector(".tool-call-card__sr")?.textContent === "Completed",
  );
}

{
  const card = new ToolCallCard(
    makeItem({
      calls: [
        { id: "call-1", name: "read_file", args: "{}" },
        { id: "call-2", name: "write_file", args: "{}" },
      ],
    }),
    { running: false },
  );
  check(
    "a mixed-name batch falls back to a generic header",
    card.element.querySelector(".tool-call-card__name")?.textContent === "Tool calls",
  );
}

{
  const card = new ToolCallCard(
    makeItem({ calls: [{ id: "call-1", name: "", args: "{}" }] }),
    { running: false },
  );
  check(
    "a batch whose only call has no name falls back to a generic header",
    card.element.querySelector(".tool-call-card__name")?.textContent === "Tool call",
  );
}

{
  const card = new ToolCallCard(
    makeItem({
      calls: [
        { id: "call-1", name: "read_file", args: "{}" },
        { id: "call-2", name: "", args: "{}" },
      ],
    }),
    { running: false },
  );
  const labels = [...card.element.querySelectorAll(".tool-call-card__call-name")];
  check(
    "a nameless call in a multi-call batch gets a generic label",
    labels[1]?.textContent === "Tool call",
  );
}

// --- Body --------------------------------------------------------------------

{
  const card = new ToolCallCard(makeItem(), { running: false });
  const args = [...card.element.querySelectorAll(".tool-call-card__args pre")];
  check(
    "the body renders one highlighted args block per call",
    args.length === 2 && args.every((pre) => pre.classList.contains("shiki")),
  );
  check(
    "an args block keeps the call's JSON text",
    args[0]?.textContent?.includes('"path"') === true &&
      args[0]?.textContent?.includes("src/main.ts") === true,
  );
  check(
    "a multi-call batch labels each call block",
    card.element.querySelectorAll(".tool-call-card__call-name").length === 2,
  );
}

{
  const card = new ToolCallCard(
    makeItem({ calls: [{ id: "call-1", name: "read_file", args: "" }] }),
    { running: false },
  );
  check(
    "a call without arguments renders no args block",
    card.element.querySelector(".tool-call-card__args") === null,
  );
}

{
  const card = new ToolCallCard(
    makeItem({
      calls: [{ id: "call-1", name: "read_file", args: '{"html":"<script>alert(1)</script>"}' }],
    }),
    { running: false },
  );
  const args = card.element.querySelector(".tool-call-card__args");
  check(
    "hostile markup in call args lands as escaped text, never live elements",
    args?.querySelector("script") === null &&
      args?.textContent?.includes("<script>") === true,
  );
}

{
  const card = new ToolCallCard(makeItem({ calls: [], text: "not json at all" }), {
    running: false,
  });
  check(
    "an unparsed batch falls back to its raw text",
    card.element.querySelector(".tool-call-card__raw")?.textContent === "not json at all",
  );
  check(
    "an unparsed batch renders no count badge",
    card.element.querySelector(".tool-call-card__count") === null,
  );
}

// --- Result ------------------------------------------------------------------

{
  const card = new ToolCallCard(makeItem(), { running: true });
  const result = card.element.querySelector(".tool-call-card__result");
  check("the result block starts hidden while running", result?.hidden === true);
  card.setResult("file contents <script>alert(1)</script>");
  check("setResult reveals the result block", result?.hidden === false);
  check(
    "the result lands as text, never markup",
    result?.textContent.includes("<script>") === true &&
      result?.querySelector("script") === null,
  );
  card.setResult(null);
  check("clearing the result hides the block again", result?.hidden === true);
}

{
  const card = new ToolCallCard(makeItem(), { running: false, result: "file contents" });
  const result = card.element.querySelector(".tool-call-card__result");
  check(
    "a result passed at construction shows without a setResult call",
    result?.hidden === false && result.textContent === "file contents",
  );
}

// --- Running drives the disclosure --------------------------------------------

{
  const card = new ToolCallCard(makeItem(), { running: true });
  check(
    "a running card auto-opens",
    card.element.open === true && card.element.classList.contains("tool-call-card--running"),
  );
  card.setRunning(false);
  check(
    "the card auto-collapses when running flips false",
    card.element.open === false && !card.element.classList.contains("tool-call-card--running"),
  );
}

{
  const card = new ToolCallCard(makeItem(), { running: false });
  check("a settled card starts collapsed", card.element.open === false);
  card.setRunning(true);
  check("setRunning(true) opens a settled card", card.element.open === true);
}

{
  const card = new ToolCallCard(makeItem(), { running: false });
  card.element.open = true; // the operator opened it by hand
  card.setRunning(false);
  check(
    "a repeated settled state never slams a hand-opened card",
    card.element.open === true,
  );
}

// --- Native disclosure ---------------------------------------------------------

{
  const card = new ToolCallCard(makeItem(), { running: false });
  document.body.appendChild(card.element);
  const summary = card.element.querySelector("summary");
  summary?.click();
  check("clicking the summary expands the card", card.element.open === true);
  summary?.click();
  check("clicking the summary again collapses the card", card.element.open === false);
  card.element.remove();
}

if (failures.length > 0) {
  console.error(`tool-call-card: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("tool-call-card: all assertions passed");
process.exit(0);
