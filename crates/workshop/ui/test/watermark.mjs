// The empty editor group's watermark (src/parts/layout/watermark.ts and
// watermark.css): a product mark over shortcut rows that read their key caps
// from the keybinding registry. The rows run New Agent, Show Files, Search
// Files, then Add Folder; a row whose command has no keybinding is left out,
// and Add Folder shows only while exactly one root is open, re-checking when
// the workspace changes - with no tree panel mounted too, because the
// tree-state service drops the shared roots load itself. The stylesheet pins
// the look: at most 500px wide,
// half opacity, a 100px mark, rows padded 4px 4px 4px 8px with a 4px radius,
// the rows dropping out below a 478px-wide group, and nothing shown in the
// side zones' empty groups.
// Run: node test/watermark.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { declaration, readUi, resolver, rulesOf, valueIn } from "./helpers/css-values.mjs";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createWatermark, WATERMARK_ROWS } from "./src/parts/layout/watermark.ts";
      export { createKeybindingsRegistry } from "@workshop/platform/keybinding-registry";
      export { WORKSPACE_CHANGED_EVENT } from "./src/services/workspace-events.ts";
      export { TREE_STATE, TreeStateService } from "./src/services/tree-state-service.ts";
      export { registerService } from "@workshop/platform/service-registry";
    `,
    resolveDir: uiDir,
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
for (const name of ["window", "document", "Element", "HTMLElement", "Node", "Event", "CustomEvent", "KeyboardEvent"]) {
  globalThis[name] = window[name];
}
const bundlePath = path.join(os.tmpdir(), "promptforge-watermark-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createWatermark,
  WATERMARK_ROWS,
  createKeybindingsRegistry,
  WORKSPACE_CHANGED_EVENT,
  TREE_STATE,
  TreeStateService,
  registerService,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}
const flush = async () => {
  for (let i = 0; i < 4; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

/** A registry with every watermark command bound except the ones left out. */
function registryWith(skip = []) {
  const registry = createKeybindingsRegistry("windows");
  const bindings = {
    "workbench.action.chat.new": "ctrl+shift+l",
    "workbench.view.explorer": "ctrl+shift+e",
    "workbench.action.quickOpen": "ctrl+p",
    "workbench.action.files.openFolder": "ctrl+m ctrl+o",
  };
  for (const [id, keybinding] of Object.entries(bindings)) {
    if (!skip.includes(id)) registry.registerKeybindingRule({ id, keybinding });
  }
  return registry;
}

/** A scripted root count the test can change between workspace events. */
function rootCounter(initial) {
  const counter = { count: initial, reads: 0 };
  counter.read = async () => {
    counter.reads += 1;
    return counter.count;
  };
  return counter;
}

const labelsOf = (watermark) =>
  [...watermark.element.querySelectorAll(".ws-watermark__row")].map(
    (row) => row.querySelector(".ws-watermark__label")?.textContent,
  );
const chordsOf = (row) =>
  [...row.querySelectorAll(".ws-watermark__chord")].map((chord) =>
    [...chord.querySelectorAll(".ws-watermark__key")].map((key) => key.textContent),
  );

// --- The row list ---------------------------------------------------------------------

check(
  "the watermark's rows run New Agent, Show Files, Search Files, Add Folder",
  WATERMARK_ROWS.map((row) => row.label).join("|") === "New Agent|Show Files|Search Files|Add Folder",
);

// --- One root open: all four rows, with their key caps ----------------------------------

{
  const roots = rootCounter(1);
  const watermark = createWatermark({ keybindings: registryWith(), rootCount: roots.read });
  watermark.init({});
  document.body.append(watermark.element);
  await flush();
  check("the watermark is a div with the watermark class", watermark.element.classList.contains("ws-watermark"));
  check(
    "the product mark is an image with no alt text",
    watermark.element.querySelector("img.ws-watermark__mark")?.getAttribute("alt") === "",
  );
  check(
    "one root shows all four rows in order",
    labelsOf(watermark).join("|") === "New Agent|Show Files|Search Files|Add Folder",
  );
  const rows = [...watermark.element.querySelectorAll(".ws-watermark__row")];
  check(
    "a row draws one key cap per key of its chord",
    JSON.stringify(chordsOf(rows[0])) === JSON.stringify([["Ctrl", "Shift", "L"]]) &&
      JSON.stringify(chordsOf(rows[2])) === JSON.stringify([["Ctrl", "P"]]),
  );
  check(
    "a two-chord keybinding draws one group of caps per chord",
    JSON.stringify(chordsOf(rows[3])) === JSON.stringify([["Ctrl", "M"], ["Ctrl", "O"]]),
  );
  check(
    "each row names its command",
    rows[1].dataset.commandId === "workbench.view.explorer",
  );

  // --- The root count is re-read when the workspace changes -----------------------------
  const readsBefore = roots.reads;
  roots.count = 2;
  window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
  await flush();
  check("a workspace change re-reads the root count", roots.reads === readsBefore + 1);
  check(
    "two roots drop the Add Folder row",
    labelsOf(watermark).join("|") === "New Agent|Show Files|Search Files",
  );
  roots.count = 1;
  window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
  await flush();
  check("back to one root, Add Folder returns", labelsOf(watermark).at(-1) === "Add Folder");

  watermark.dispose?.();
  const readsAfterDispose = roots.reads;
  window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
  await flush();
  check("a disposed watermark stops listening for workspace changes", roots.reads === readsAfterDispose);
  watermark.element.remove();
}

// --- No root, and the first paint before the count is known --------------------------------

{
  const roots = rootCounter(0);
  const watermark = createWatermark({ keybindings: registryWith(), rootCount: roots.read });
  watermark.init({});
  check(
    "before the count arrives the conditional row is not shown",
    labelsOf(watermark).join("|") === "New Agent|Show Files|Search Files",
  );
  await flush();
  check(
    "no root shows no Add Folder row",
    labelsOf(watermark).join("|") === "New Agent|Show Files|Search Files",
  );
  watermark.dispose?.();
}

// --- A failing count read leaves the row out ----------------------------------------------------

{
  const watermark = createWatermark({
    keybindings: registryWith(),
    rootCount: async () => {
      throw new Error("server down");
    },
  });
  watermark.init({});
  await flush();
  check("an unreadable root count leaves Add Folder out", labelsOf(watermark).at(-1) === "Search Files");
  watermark.dispose?.();
}

// --- Only commands that have a keybinding get a row ------------------------------------------------

{
  const watermark = createWatermark({
    keybindings: registryWith(["workbench.action.quickOpen"]),
    rootCount: rootCounter(1).read,
  });
  watermark.init({});
  await flush();
  check(
    "a command without a keybinding has no row",
    labelsOf(watermark).join("|") === "New Agent|Show Files|Add Folder",
  );
  watermark.dispose?.();
}

// --- A workspace change with no tree panel mounted ---------------------------------------------------

// The empty editor's watermark is on screen exactly when Explorer is closed,
// so no tree panel is alive to drop the shared roots load. The tree-state
// service follows the event itself (main.ts installs it); here the watermark
// reads the real service through its default root count, over a scripted
// /workspace/tree, and the only listener besides its own is the service's.
{
  const tree = new TreeStateService();
  registerService(TREE_STATE, () => tree);
  const follower = tree.followWorkspaceChanges(window);
  const root = (name) => ({ name, path: `C:\\${name}`, kind: "directory", size: 0, modified_ms: 1, exists: true });
  let listing = { path: null, entries: [root("one")] };
  let treeReads = 0;
  globalThis.fetch = async (url) => {
    if (url !== "/workspace/tree") throw new Error(`unexpected fetch in the watermark test: ${url}`);
    treeReads += 1;
    return { ok: true, status: 200, json: async () => listing };
  };
  const watermark = createWatermark({ keybindings: registryWith() });
  watermark.init({});
  document.body.append(watermark.element);
  await flush();
  check(
    "the real roots load shows Add Folder while one root is granted",
    labelsOf(watermark).at(-1) === "Add Folder" && treeReads === 1,
  );

  listing = { path: null, entries: [root("one"), root("two")] };
  window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
  await flush();
  check("a root granted with no tree panel mounted re-reads the roots", treeReads === 2);
  check(
    "a root granted with no tree panel mounted drops the Add Folder row",
    labelsOf(watermark).join("|") === "New Agent|Show Files|Search Files",
  );

  listing = { path: null, entries: [root("one")] };
  window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT));
  await flush();
  check("a root removed with no tree panel mounted brings Add Folder back", labelsOf(watermark).at(-1) === "Add Folder" && treeReads === 3);

  window.dispatchEvent(new window.CustomEvent(WORKSPACE_CHANGED_EVENT, { detail: { rootsCurrent: true } }));
  await flush();
  check("an event saying the roots are current fetches nothing", treeReads === 3);

  watermark.dispose();
  watermark.element.remove();
  follower.dispose();
  tree.dispose();
}

// --- The stylesheet ----------------------------------------------------------------------------------

{
  const resolve = await resolver();
  const css = await readUi("src/parts/layout/watermark.css");
  const rules = rulesOf(css);
  const value = (selector, property, at = null) => resolve(valueIn(rules, selector, property, at));
  check("the watermark is at most 500px wide", value(".ws-watermark", "max-inline-size") === "500px");
  check("the watermark sits at half opacity", value(".ws-watermark", "opacity") === "0.5");
  check("the watermark text is the quiet #F0F0F099", value(".ws-watermark", "color") === "#f0f0f099");
  check("the watermark measures its own width for the row cutoff", value(".ws-watermark", "container-type") === "inline-size");
  check("the product mark is 100px", value(".ws-watermark__mark", "inline-size") === "100px");
  check(
    "a row is padded 4px 4px 4px 8px",
    value(".ws-watermark__row", "padding") === "4px 4px 4px 8px",
  );
  check("a row has a 4px radius", value(".ws-watermark__row", "border-radius") === "4px");
  const cutoff = rules.find(
    (rule) =>
      rule.selectors.includes(".ws-watermark__rows") &&
      rule.at.some((header) => /^@container\s*\(\s*(width\s*<\s*478px|max-width:\s*477\.98px)\s*\)$/.test(header)),
  );
  check(
    "the rows are hidden in a group narrower than 478px",
    cutoff !== undefined && declaration(cutoff.body, "display") === "none",
  );
  const hiddenInSides = rules.find(
    (rule) =>
      rule.selectors.includes('[data-ws-zone="left"] .ws-watermark') &&
      rule.selectors.includes('[data-ws-zone="right"] .ws-watermark'),
  );
  check(
    "the side zones' empty groups show no watermark",
    hiddenInSides !== undefined && declaration(hiddenInSides.body, "display") === "none",
  );
}

if (failures.length > 0) {
  console.error(`watermark: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("watermark: all assertions passed");
process.exit(0);
