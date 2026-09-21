// Integration test for the Run window panel (src/parts/run/, the run tab's
// loading shimmer in src/parts/layout/run-tab.ts, the tree's drag-out in
// workshop-panel.ts, and the drop-target dispatch in workspace-drops.ts).
// Bundles the modules with esbuild, mounts a real Dockview dock in jsdom,
// and scripts fetch for /workspace/tree, /workspace/file, /prompts/contract,
// and /workspace/grant. Covers: the empty open; a pre-filled open reaching
// ready with one row per contract item; the shimmer class on the tab title
// in loading and its absence in ready and error; tree dragstart setting
// application/x-workshop-path and a tree drop loading the prompt; an OS
// drop granting then loading the first .md; a Browse pick granted then
// loading to ready; a parse failure rendering the
// line-numbered error row with Choose Prompt; a contract declaring the
// retired fuzzy tool shape rejected as an unexpected shape; two opens
// yielding two windows; and a superseded load discarded by the generation
// counter.
// Run: node --test test/run-panel.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createDockview, themeDark } from "dockview";
      export { initZones, openInZone, panelIdFor, zoneOfPanel } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/parts/layout/panel-types.ts";
      export { setupWorkspaceDrops } from "./src/parts/workspace/workspace-drops.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/plugin-dialog": path.join(uiDir, "helpers", "tauri-dialog-stub.mjs"),
  },
});

const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
const { window } = dom;

window.matchMedia =
  window.matchMedia ||
  (() => ({
    matches: false,
    media: "",
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  }));
window.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
window.IntersectionObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
};
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};

// The agent panel composes an AgentSocket on init; a scripted stand-in
// that never opens keeps the panel inert.
globalThis.WebSocket = class {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  readyState = 0;
  send() {}
  close() {}
};

// --- The scripted workspace and prompt routes --------------------------------

const ROOT = "C:\\project";
const PROMPT = `${ROOT}\\prompt.md`;
const BROKEN = `${ROOT}\\broken.md`;
const SLOW = `${ROOT}\\slow.md`;
const FAST = `${ROOT}\\fast.md`;
const OSDROP = `${ROOT}\\osdrop.md`;
const FUZZY = `${ROOT}\\fuzzy.md`;

const calls = [];
// Paths whose /workspace/file answer pends until releaseDeferred() runs.
const deferred = new Map();

function contractFor(name) {
  return {
    name,
    description: `contract for ${name}`,
    promptforge: 1,
    max_tool_iterations: 12,
    input: { path: "in/papers.md", description: "the papers" },
    output: { path: "out/verdicts.md", description: "the verdicts" },
    capabilities: [{ id: "tools/web", optional: false }],
    tools: [{ kind: "exact", alias: "search", path: "tools/web/search" }],
    args: {
      implicit: false,
      fields: [
        { name: "topic", type: "string", optional: false, default: null, description: "the topic" },
        { name: "deep", type: "boolean", optional: true, default: false, description: null },
        { name: "limit", type: "integer", optional: true, default: 5, description: null },
      ],
    },
    models: [
      { label: "main", keywords: ["thinking", "frontier"], min_context: 32000, description: null },
    ],
  };
}

const json = (body, status = 200) => ({
  ok: status >= 200 && status < 300,
  status,
  json: async () => body,
});

globalThis.fetch = (url, init) => {
  const target = typeof url === "string" ? url : url.url;
  const method = init?.method ?? "GET";
  calls.push({ url: target, method });
  if (target.startsWith("/workspace/tree")) {
    const query = target.includes("?") ? new URL(target, "http://127.0.0.1:7910").searchParams : null;
    const pathParam = query ? query.get("path") : null;
    if (pathParam === null || pathParam === "") {
      return Promise.resolve(
        json({ path: null, entries: [{ name: "project", path: ROOT, kind: "directory", size: 0, modified_ms: 100, exists: true }] }),
      );
    }
    if (pathParam === ROOT) {
      return Promise.resolve(
        json({
          path: ROOT,
          entries: [
            { name: "prompt.md", path: PROMPT, kind: "file", size: 3, modified_ms: 100, exists: true },
            { name: "slow.md", path: SLOW, kind: "file", size: 3, modified_ms: 100, exists: true },
            { name: "fast.md", path: FAST, kind: "file", size: 3, modified_ms: 100, exists: true },
          ],
        }),
      );
    }
  }
  if (target.startsWith("/workspace/file")) {
    const pathParam = new URL(target, "http://127.0.0.1:7910").searchParams.get("path");
    if (deferred.has(pathParam)) {
      return new Promise((resolve) => deferred.set(pathParam, resolve));
    }
    return Promise.resolve(
      json({ path: pathParam, size: 7, token: "t100", text: `text of ${pathParam}` }),
    );
  }
  if (target === "/prompts/contract" && method === "POST") {
    const name = JSON.parse(init.body).name;
    if (name.includes("broken")) {
      return Promise.resolve(
        json({ error: { code: "parse_frontmatter", message: "line 3: bad YAML key" } }, 422),
      );
    }
    if (name.includes("fuzzy")) {
      // The retired fuzzy slot shape: the contract parser must refuse it.
      const contract = contractFor(name);
      return Promise.resolve(
        json({
          ...contract,
          tools: [
            ...contract.tools,
            { kind: "fuzzy", alias: "fetch", want: "fetch a page", optional: true },
          ],
        }),
      );
    }
    return Promise.resolve(json(contractFor(name)));
  }
  if (target === "/workspace/grant" && method === "POST") {
    return Promise.resolve(json({ granted: JSON.parse(init.body).path }));
  }
  throw new Error(`unexpected fetch in the run-panel test: ${target}`);
};

function releaseDeferred(path) {
  const resolve = deferred.get(path);
  deferred.delete(path);
  resolve?.({
    ok: true,
    status: 200,
    json: async () => ({ path, size: 7, token: "t100", text: `text of ${path}` }),
  });
}

for (const key of [
  "document",
  "navigator",
  "location",
  "localStorage",
  "HTMLElement",
  "HTMLTemplateElement",
  "HTMLInputElement",
  "HTMLTextAreaElement",
  "HTMLButtonElement",
  "Node",
  "Element",
  "Event",
  "CustomEvent",
  "MutationObserver",
  "Option",
  "DOMParser",
  "ResizeObserver",
  "IntersectionObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;
globalThis.window = window;
globalThis.document = window.document;

const bundlePath = path.join(os.tmpdir(), "run-panel-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  initZones,
  openInZone,
  panelIdFor,
  createPanelComponent,
  createPanelTabComponent,
  setupWorkspaceDrops,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// jsdom has no DragEvent; a plain Event with a scripted dataTransfer is
// enough for the handlers' types/getData probes.
function syntheticDrag(type, target, dataTransfer) {
  const event = new window.Event(type, { cancelable: true, bubbles: true });
  Object.defineProperty(event, "dataTransfer", { value: dataTransfer });
  target.dispatchEvent(event);
  return event;
}

// The run panel's element, once the lazy chunk has swapped it in.
function runElement(panel) {
  return panel.view.content.element.querySelector(".ws-run-panel");
}

const dock = createDockview(window.document.getElementById("dock"), {
  createComponent: createPanelComponent,
  createTabComponent: createPanelTabComponent,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
initZones(dock);

// The tree anchors the left zone and provides the drag source rows.
openInZone("tree", {});
await flush();

// --- Empty open: the prompt field, Browse, and the drop zone ----------------

const emptyRun = openInZone("run", { instance: "empty" });
await flush();
const emptyEl = runElement(emptyRun);
check("an empty Run window mounts", !!emptyEl);
check("the empty window shows the prompt field", !!emptyEl?.querySelector(".ws-run-panel__prompt-path"));
check("the empty window shows a Browse button", !!emptyEl?.querySelector(".ws-run-panel__browse"));
check("the panel root is the file-drop target", emptyEl?.hasAttribute("data-ws-file-drop") === true);
check("the empty window shows the empty-state hint", !!emptyEl?.querySelector(".ws-run-panel__hint"));
check("the Run button is disabled before ready", emptyEl?.querySelector(".ws-run-panel__run")?.disabled === true);
check("an empty open fetches nothing", calls.filter((c) => c.url.startsWith("/workspace/file")).length === 0);

// --- Pre-filled open: loading shimmers, then one row per contract item ------

deferred.set(PROMPT, null);
const filledRun = openInZone("run", { instance: "filled", path: PROMPT });
await flush();
const filledEl = runElement(filledRun);
check("a pre-filled window mounts", !!filledEl);
const filledTabContent = filledRun.view.tab.element.querySelector(".dv-default-tab-content");
check(
  "the tab title shimmers while loading",
  filledTabContent?.classList.contains("ws-shimmer-text") === true,
);
check("the body stays blank while loading", filledEl?.querySelector(".ws-run-panel__rows") === null);
releaseDeferred(PROMPT);
await flush();
check(
  "the shimmer clears on ready",
  filledTabContent?.classList.contains("ws-shimmer-text") === false,
);
check(
  "the file read precedes the contract post",
  calls.some((c) => c.url.startsWith("/workspace/file")) &&
    calls.some((c) => c.url === "/prompts/contract" && c.method === "POST"),
);
check("the title names the loaded prompt", filledRun.title === "Run: prompt.md");

const rows = [...filledEl.querySelectorAll(".ws-run-panel__row")];
const rowText = (label) =>
  rows.find((row) => row.querySelector(".ws-run-panel__row-label")?.textContent === label);
check("the name renders as a read-only row", rowText("name")?.textContent.includes("prompt.md") === true);
check(
  "the description renders as a read-only row",
  rowText("description")?.textContent.includes("contract for prompt.md") === true,
);
const inputField = rowText("input")?.querySelector("input");
check(
  "the input row is a text field prefilled from the contract, with Browse",
  inputField?.value === "in/papers.md" && !!rowText("input")?.querySelector("button"),
);
const outputField = rowText("output")?.querySelector("input");
check("the output row is a prefilled text field", outputField?.value === "out/verdicts.md");
const topicRow = rowText("topic");
check(
  "a required arg is marked required",
  topicRow?.querySelector(".ws-run-panel__required") !== null &&
    topicRow?.querySelector("input") !== null,
);
const deepRow = rowText("deep");
check(
  "a boolean arg is a checkbox from its default",
  deepRow?.querySelector('input[type="checkbox"]') !== null &&
    deepRow.querySelector('input[type="checkbox"]').checked === false,
);
const limitRow = rowText("limit");
check(
  "an integer arg is a numeric control prefilled from its default",
  limitRow?.querySelector('input[type="number"]')?.value === "5" &&
    limitRow.querySelector('input[type="number"]')?.step === "1",
);
const capabilityRow = rowText("tools/web");
check(
  "a required capability is a disabled checkbox",
  capabilityRow?.querySelector('input[type="checkbox"]')?.disabled === true,
);
const toolRows = rows.filter((row) => row.classList.contains("ws-run-panel__row--tool"));
check(
  "an exact tool renders read-only under its alias",
  toolRows.length === 1 &&
    toolRows[0].querySelector(".ws-run-panel__row-label")?.textContent === "search" &&
    toolRows[0].textContent.includes("exact: tools/web/search"),
);
const modelRow = rowText("main");
check(
  "a model role renders keywords and min_context read-only",
  modelRow?.textContent.includes("thinking") === true &&
    modelRow?.textContent.includes("frontier") === true &&
    modelRow?.textContent.includes("32000"),
);
const iterationsField = rowText("max_tool_iterations")?.querySelector('input[type="number"]');
check("max_tool_iterations prefills from the contract", iterationsField?.value === "12");

const fetchesBeforeRun = calls.length;
const runButton = filledEl.querySelector(".ws-run-panel__run");
check("the Run button is enabled in ready", runButton?.disabled === false);
runButton.click();
await flush();
check("the Run button does nothing when clicked", calls.length === fetchesBeforeRun);

// --- Tree drag: dragstart sets the MIME type; a drop loads the prompt -------

const projectRow = [...window.document.querySelectorAll(".ws-workshop-tree__row")].find(
  (row) => row.textContent === "project",
);
projectRow.click();
await flush();
const fileRow = [...window.document.querySelectorAll(".ws-workshop-tree__row")].find(
  (row) => row.textContent === "prompt.md",
);
check("the tree file row is draggable", fileRow?.draggable === true);
const drags = [];
syntheticDrag("dragstart", fileRow, {
  setData: (type, value) => drags.push({ type, value }),
});
check(
  "a tree dragstart sets application/x-workshop-path to the entry path",
  drags.length === 1 && drags[0].type === "application/x-workshop-path" && drags[0].value === PROMPT,
);

// Dropping the tree path onto the empty window loads it.
const treeDrop = syntheticDrag("drop", emptyEl.querySelector(".ws-run-panel__body"), {
  types: ["application/x-workshop-path"],
  getData: (type) => (type === "application/x-workshop-path" ? PROMPT : ""),
});
await flush();
check("a tree drop on the zone is accepted", treeDrop.defaultPrevented === true);
check(
  "a tree drop loads the prompt to ready",
  emptyEl.querySelector(".ws-run-panel__rows") !== null &&
    emptyEl.querySelector(".ws-run-panel__run")?.disabled === false,
);

// --- OS drop: grant first, then the first .md loads --------------------------

window.__TAURI_INTERNALS__ = {};
const statusMessages = [];
setupWorkspaceDrops({ showLocal: (label, severity) => statusMessages.push({ label, severity }) });

const osRun = openInZone("run", { instance: "osdrop" });
await flush();
const osEl = runElement(osRun);
const targetDrops = [];
osEl.addEventListener("workshop:file-drop", (event) => targetDrops.push(event.detail.paths));
const grantsBefore = calls.filter((c) => c.url === "/workspace/grant").length;
syntheticDrag("drop", osEl.querySelector(".ws-run-panel__body"), { types: ["Files"] });
window.dispatchEvent(
  new window.CustomEvent("promptforge:file-drop", {
    detail: { paths: [`${ROOT}\\notes.txt`, OSDROP] },
  }),
);
await flush();
const grants = calls.filter((c) => c.url === "/workspace/grant").length - grantsBefore;
check("an OS drop grants every path before the read", grants === 2);
check(
  "the drop target receives the workshop:file-drop dispatch after granting",
  targetDrops.length === 1 && targetDrops[0].join("|") === `${ROOT}\\notes.txt|${OSDROP}`,
);
check(
  "the first .md of an OS drop loads to ready",
  osEl.querySelector(".ws-run-panel__rows") !== null &&
    [...osEl.querySelectorAll(".ws-run-panel__row")].some((row) =>
      row.textContent.includes("osdrop.md"),
    ),
);

// --- Browse: the picked path is granted, then loads to ready -----------------

// __TAURI_INTERNALS__ is set (the OS-drop section), so Browse takes the
// native-picker path, answered by the aliased tauri-dialog stub.
const BROWSED = `${ROOT}\\browsed.md`;
const browseRun = openInZone("run", { instance: "browse" });
await flush();
const browseEl = runElement(browseRun);
window.__TAURI_DIALOG__ = { calls: [], answer: BROWSED };
const grantsBeforeBrowse = calls.filter((c) => c.url === "/workspace/grant").length;
browseEl.querySelector(".ws-run-panel__browse").click();
await flush();
check(
  "Browse opens the native picker filtered to .md",
  window.__TAURI_DIALOG__.calls.length === 1 &&
    window.__TAURI_DIALOG__.calls[0].kind === "open" &&
    window.__TAURI_DIALOG__.calls[0].filters?.[0]?.extensions?.includes("md") === true,
);
const browsedGrant = calls.findIndex((c, i) => i >= grantsBeforeBrowse && c.url === "/workspace/grant");
const browsedRead = calls.findIndex(
  (c) => c.url.startsWith("/workspace/file") && c.url.includes("browsed.md"),
);
check(
  "a Browse pick is granted before the read",
  browsedGrant !== -1 && browsedRead !== -1 && browsedGrant < browsedRead,
);
check(
  "a Browse pick loads the prompt to ready",
  browseEl.querySelector(".ws-run-panel__rows") !== null &&
    [...browseEl.querySelectorAll(".ws-run-panel__row")].some((row) =>
      row.textContent.includes("browsed.md"),
    ) &&
    browseEl.querySelector(".ws-run-panel__run")?.disabled === false,
);

// --- Parse failure: the line-numbered error row and Choose Prompt -----------

const brokenRun = openInZone("run", { instance: "broken", path: BROKEN });
await flush();
const brokenEl = runElement(brokenRun);
const errorRow = brokenEl?.querySelector(".ws-run-panel__error");
check("a parse failure renders the error row", !!errorRow);
check(
  "the error row shows the server's line-numbered message",
  errorRow?.textContent.includes("line 3: bad YAML key") === true,
);
check("the error row is announced as an alert", errorRow?.getAttribute("role") === "alert");
check("the error state offers Choose Prompt", !!brokenEl?.querySelector(".ws-run-panel__choose"));
check(
  "the shimmer clears on error",
  brokenRun.view.tab.element
    .querySelector(".dv-default-tab-content")
    ?.classList.contains("ws-shimmer-text") === false,
);
check("the Run button stays disabled in error", brokenEl?.querySelector(".ws-run-panel__run")?.disabled === true);

// --- A fuzzy tool entry is not a contract shape the panel accepts ------------

const fuzzyRun = openInZone("run", { instance: "fuzzy", path: FUZZY });
await flush();
const fuzzyEl = runElement(fuzzyRun);
const fuzzyError = fuzzyEl?.querySelector(".ws-run-panel__error");
check(
  "a contract declaring a fuzzy tool is rejected as an unexpected shape",
  fuzzyError?.textContent.includes("unexpected shape") === true &&
    fuzzyEl?.querySelector(".ws-run-panel__rows") === null,
);

// --- Two opens yield two windows ---------------------------------------------

const first = openInZone("run", { instance: "one" });
const second = openInZone("run", { instance: "two" });
check(
  "two opens produce two windows with distinct ids",
  first !== second && first.id === "run:one" && second.id === "run:two",
);
check("run panel ids key by instance", panelIdFor("run", { instance: "one" }) === "run:one");

// --- A superseded load is discarded by the generation counter ----------------

const genRun = openInZone("run", { instance: "gen" });
await flush();
const genEl = runElement(genRun);
// The slow load pends at the file read; the fast load lands first and
// must win even after the slow read resolves.
deferred.set(SLOW, null);
syntheticDrag("drop", genEl.querySelector(".ws-run-panel__body"), {
  types: ["application/x-workshop-path"],
  getData: () => SLOW,
});
syntheticDrag("drop", genEl.querySelector(".ws-run-panel__body"), {
  types: ["application/x-workshop-path"],
  getData: () => FAST,
});
await flush();
releaseDeferred(SLOW);
await flush();
const genRows = [...genEl.querySelectorAll(".ws-run-panel__row")].map((row) => row.textContent);
check(
  "a superseded load never renders",
  genRows.some((text) => text.includes("contract for fast.md")) &&
    !genRows.some((text) => text.includes("contract for slow.md")),
);

// --- Reduced motion: the shimmer degrades to a static muted title ------------

// jsdom applies no stylesheets, so the reduced-motion contract is checked
// against the stylesheet itself: under prefers-reduced-motion the
// animation, gradient, clip, and transparent fill all come off.
const shimmerCss = await readFile(
  path.join(uiDir, "..", "..", "..", "shared-ui", "shimmer.css"),
  "utf8");
const reducedBlock = shimmerCss.split("@media (prefers-reduced-motion: reduce)")[1] ?? "";
check("the shimmer class is defined", shimmerCss.includes(".ws-shimmer-text"));
check("the shimmer has a reduced-motion block", reducedBlock.length > 0);
check(
  "reduced motion strips the animation, gradient, clip, and transparent fill",
  reducedBlock.includes("animation: none") &&
    reducedBlock.includes("background-image: none") &&
    reducedBlock.includes("background-clip: unset") &&
    reducedBlock.includes("-webkit-text-fill-color: unset"),
);

if (failures.length > 0) {
  console.error(`run-panel: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("run-panel: all assertions passed");
process.exit(0);
