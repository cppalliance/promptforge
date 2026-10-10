// Integration test for the Run window panel (src/parts/run/, the tab's
// loading shimmer through setTabLoading in src/parts/layout/panel-tab.ts,
// the tree's drag-out in
// workshop-panel.ts, and the drop-target dispatch in workspace-drops.ts).
// Bundles the modules with esbuild, mounts a real Dockview dock in jsdom,
// and scripts fetch for /workspace/tree, /workspace/file, /prompts/contract,
// and /workspace/grant. Covers: the empty open, and the Run tab title for
// an empty path; a pre-filled open reaching
// ready with one row per contract item and no tool row; the shimmer class on the tab title
// in loading and its absence in ready and error, including after the
// overflow list inits a second tab for the panel; tree dragstart setting
// application/x-workshop-path and a tree drop loading the prompt; an OS
// drop granting then loading the first .md; a Browse pick granted then
// loading to ready; a parse failure rendering the
// line-numbered error row with Choose Prompt; two opens
// yielding two windows; and a superseded load discarded by the generation
// counter.
// The Cursor Settings layout is covered too: an arg's description as a
// visible second line (never a tooltip), a boolean arg as a .switch, the
// screen-reader-only Prompt label tied to its field, Browse... and Choose
// Prompt as inline links, the drag-over class a droppable drag sets, Choose
// Prompt and Choose Input opening the form modal with Cancel first, and the
// stylesheet's values (the 4% card, the 12px rows, the stacked layout under
// 500px, the 35px toolbar, the 22px state lines, the footer), read from the
// source because jsdom applies no layout.
// Run: node --test test/run-panel.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { readUi, resolver, rulesOf, valueIn } from "./helpers/css-values.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      // The panel types this test opens register from their contributions.
      import "./src/parts/workspace/workspace.contribution.ts";
      import "./src/parts/run/run.contribution.ts";
      export { createDockview, themeDark } from "dockview";
      export { initZones, openInZone, panelIdFor, zoneOfPanel } from "./src/parts/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent, PANEL_TAB } from "./src/parts/layout/panel-types.ts";
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
    plugins: [{ id: "tools/web" }],
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
  PANEL_TAB,
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
  defaultTabComponent: PANEL_TAB,
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
const hintLink = emptyEl?.querySelector(".ws-run-panel__hint .ws-run-panel__link");
check(
  "the empty state offers Browse... as an inline link in the hint",
  emptyEl?.querySelector(".ws-run-panel__hint")?.textContent === "Drop a prompt file here, or Browse..." &&
    hintLink?.tagName === "BUTTON" &&
    hintLink.textContent === "Browse..." &&
    !hintLink.classList.contains("button"),
);
const promptLabel = emptyEl?.querySelector(".ws-run-panel__prompt-label");
const promptPath = emptyEl?.querySelector(".ws-run-panel__prompt-path");
check(
  "the Prompt label is a real label tied to the path field",
  promptLabel?.tagName === "LABEL" &&
    promptLabel.textContent === "Prompt" &&
    typeof promptPath?.id === "string" &&
    promptPath.id.length > 0 &&
    promptLabel.htmlFor === promptPath.id,
);
check(
  "the toolbar lists the label, the path field, then Browse",
  [...(emptyEl?.querySelector(".ws-run-panel__toolbar")?.children ?? [])]
    .map((child) => child.className.split(" ").find((name) => name.startsWith("ws-run-panel__")))
    .join(",") === "ws-run-panel__prompt-label,ws-run-panel__prompt-path,ws-run-panel__browse",
);
check("the Run button is disabled before ready", emptyEl?.querySelector(".ws-run-panel__run")?.disabled === true);
check("an empty open fetches nothing", calls.filter((c) => c.url.startsWith("/workspace/file")).length === 0);

const blankRun = openInZone("run", { instance: "blank", path: "" });
await flush();
check(
  "a Run window opened with an empty path shows the Run tab title",
  blankRun.title === "Run" &&
    blankRun.view.tab.element.querySelector(".dv-default-tab-content")?.textContent === "Run",
);
check(
  "two windows give their path fields distinct ids",
  runElement(blankRun)?.querySelector(".ws-run-panel__prompt-path")?.id !== promptPath?.id &&
    runElement(blankRun)?.querySelector(".ws-run-panel__prompt-label")?.htmlFor ===
      runElement(blankRun)?.querySelector(".ws-run-panel__prompt-path")?.id,
);

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
// The overflow list's row renderer: a second tab for the same panel id,
// which Dockview never disposes.
const overflowTab = filledRun.view.createTabRenderer("headerOverflow");
check(
  "an overflow copy of the tab picks up the loading shimmer",
  overflowTab.element.querySelector(".dv-default-tab-content")?.classList.contains("ws-shimmer-text") === true,
);
releaseDeferred(PROMPT);
await flush();
check(
  "the shimmer clears on ready, on the header tab and not its overflow copy",
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
const topicDescription = topicRow?.querySelector(".ws-run-panel__row-description");
check(
  "an arg's description is a visible second line under its label",
  topicDescription?.textContent === "the topic" &&
    topicDescription.parentElement === topicRow.querySelector(".ws-run-panel__row-label")?.parentElement?.parentElement &&
    topicRow.querySelector(".ws-run-panel__row-text") === topicDescription.parentElement,
);
check(
  "an arg's description is not also a tooltip",
  topicRow?.querySelector("input")?.getAttribute("title") === null &&
    topicRow.getAttribute("title") === null,
);
const deepRow = rowText("deep");
const deepSwitch = deepRow?.querySelector("button.switch");
check(
  "a boolean arg is a switch from its default, not a checkbox",
  deepSwitch?.getAttribute("role") === "switch" &&
    deepSwitch.getAttribute("type") === "button" &&
    deepSwitch.getAttribute("aria-checked") === "false" &&
    deepRow.querySelector('input[type="checkbox"]') === null,
);
deepSwitch?.click();
check("clicking a switch turns it on", deepSwitch?.getAttribute("aria-checked") === "true");
deepSwitch?.click();
check("clicking it again turns it off", deepSwitch?.getAttribute("aria-checked") === "false");
const limitRow = rowText("limit");
check(
  "an arg without a description has no second line",
  deepRow?.querySelector(".ws-run-panel__row-description") === null &&
    limitRow?.querySelector(".ws-run-panel__row-description") === null,
);
check(
  "an integer arg is a numeric control prefilled from its default",
  limitRow?.querySelector('input[type="number"]')?.value === "5" &&
    limitRow.querySelector('input[type="number"]')?.step === "1",
);
check(
  "the ready panel renders no tool row",
  rows.length > 0 && filledEl.querySelector(".ws-run-panel__row--tool") === null,
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

// The content column holds the toolbar, body, and footer, and the panel's
// dialogs mount beside it, so the column's width container never contains them.
check(
  "the content column holds the toolbar, the body, and the footer",
  [...(emptyEl.querySelector(".ws-run-panel__content")?.children ?? [])]
    .map((child) => child.className)
    .join(",") === "ws-run-panel__toolbar,ws-run-panel__body,ws-run-panel__footer",
);

// --- Drag over: a droppable drag marks the panel with the overlay class -----

const OVER = "ws-run-panel--drag-over";
const dragRun = openInZone("run", { instance: "dragover" });
await flush();
const dragEl = runElement(dragRun);
const dragBody = dragEl.querySelector(".ws-run-panel__body");
const dragHint = dragEl.querySelector(".ws-run-panel__hint");
const TREE_DRAG = { types: ["application/x-workshop-path"], getData: () => "" };
check("a panel at rest carries no drag-over class", !dragEl.classList.contains(OVER));
syntheticDrag("dragenter", dragBody, TREE_DRAG);
check("a tree drag entering marks the panel", dragEl.classList.contains(OVER));
syntheticDrag("dragenter", dragHint, TREE_DRAG);
syntheticDrag("dragleave", dragBody, TREE_DRAG);
check("moving onto a child keeps the mark", dragEl.classList.contains(OVER));
syntheticDrag("dragleave", dragHint, TREE_DRAG);
check("leaving the panel clears the mark", !dragEl.classList.contains(OVER));
syntheticDrag("dragenter", dragBody, { types: ["Files"] });
check("an OS file drag entering marks the panel", dragEl.classList.contains(OVER));
syntheticDrag("drop", dragBody, { types: ["Files"], getData: () => "" });
check("a drop clears the mark", !dragEl.classList.contains(OVER));
syntheticDrag("dragenter", dragBody, { types: ["text/plain"] });
check("an unrelated drag never marks the panel", !dragEl.classList.contains(OVER));
syntheticDrag("dragleave", dragBody, { types: ["text/plain"] });
syntheticDrag("dragenter", dragBody, TREE_DRAG);
syntheticDrag("drop", dragBody, { types: ["application/x-workshop-path"], getData: () => FAST });
await flush();
check(
  "a tree drop clears the mark and still loads the prompt",
  !dragEl.classList.contains(OVER) && dragEl.querySelector(".ws-run-panel__rows") !== null,
);

// A drag that ends with no drop on the panel (cancelled, or released
// elsewhere) is the source row's dragend bubbling up to the window. It
// clears the mark and the enter/leave depth count, so the next drag starts
// from zero instead of needing extra leaves to clear.
const dragEndRun = openInZone("run", { instance: "dragend" });
await flush();
const dragEndEl = runElement(dragEndRun);
const dragEndBody = dragEndEl.querySelector(".ws-run-panel__body");
const dragEndHint = dragEndEl.querySelector(".ws-run-panel__hint");
syntheticDrag("dragenter", dragEndBody, TREE_DRAG);
syntheticDrag("dragenter", dragEndHint, TREE_DRAG);
check("a tree drag over a child marks the panel", dragEndEl.classList.contains(OVER));
syntheticDrag("dragend", fileRow, TREE_DRAG);
check("a drag that ends over the panel clears the mark", !dragEndEl.classList.contains(OVER));
syntheticDrag("dragenter", dragEndBody, TREE_DRAG);
check("a new drag marks the panel again after a drag ended", dragEndEl.classList.contains(OVER));
syntheticDrag("dragleave", dragEndBody, TREE_DRAG);
check(
  "a drag that ended leaves no depth behind: one leave clears the next mark",
  !dragEndEl.classList.contains(OVER),
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
const chooseLink = brokenEl?.querySelector(".ws-run-panel__choose");
const errorBlock = errorRow?.closest(".ws-run-panel__message") ?? null;
check(
  "Choose Prompt is an inline link beside the error text, not a button",
  chooseLink?.tagName === "BUTTON" &&
    chooseLink.textContent === "Choose Prompt" &&
    chooseLink.classList.contains("ws-run-panel__link") &&
    !chooseLink.classList.contains("button") &&
    errorBlock !== null &&
    errorBlock === chooseLink.closest(".ws-run-panel__message"),
);
check(
  "the alert carries the server's message and not the link's text",
  errorRow?.textContent.includes("line 3: bad YAML key") === true &&
    !errorRow.textContent.includes("Choose Prompt"),
);
check(
  "the shimmer clears on error",
  brokenRun.view.tab.element
    .querySelector(".dv-default-tab-content")
    ?.classList.contains("ws-shimmer-text") === false,
);
check("the Run button stays disabled in error", brokenEl?.querySelector(".ws-run-panel__run")?.disabled === true);

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

// --- Dialogs: Choose Prompt and Choose Input open the form modal, Cancel first

// A plain browser has no native picker, so Browse takes the typed-path dialog.
delete window.__TAURI_INTERNALS__;

const dialogButtons = (dialog) =>
  [...dialog.querySelectorAll(".modal-actions button")].map((button) => button.textContent);

const dialogRun = openInZone("run", { instance: "dialogs" });
await flush();
const dialogEl = runElement(dialogRun);
dialogEl.querySelector(".ws-run-panel__hint .ws-run-panel__link").click();
await flush();
const chooseDialog = dialogEl.querySelector(".ws-run-choose");
check("the Browse... link opens Choose Prompt", chooseDialog !== null);
check(
  "Choose Prompt uses the form modal skin",
  chooseDialog?.classList.contains("modal-dialog--form") === true &&
    dialogEl.querySelector(".ws-run-choose-overlay")?.classList.contains("modal-overlay--form") === true,
);
check(
  "a dialog mounts beside the content column, outside the width container",
  dialogEl.querySelector(".ws-run-choose-overlay")?.parentElement === dialogEl &&
    dialogEl.querySelector(".ws-run-panel__content .ws-run-choose-overlay") === null,
);
check(
  "Choose Prompt puts Cancel first and the primary Choose last",
  dialogButtons(chooseDialog).join(",") === "Cancel,Choose" &&
    chooseDialog.querySelector(".modal-actions button:last-child")?.classList.contains("button-primary") === true,
);
const chooseButton = chooseDialog.querySelector(".modal-actions button:last-child");
check("Choose waits for a path", chooseButton.disabled === true);
const chooseField = chooseDialog.querySelector("input");
chooseField.value = `${ROOT}\\typed.md`;
chooseField.dispatchEvent(new window.Event("input", { bubbles: true }));
check("Choose enables once a path is typed", chooseButton.disabled === false);
chooseButton.click();
await flush();
check("Choose closes the dialog and loads the typed prompt", dialogEl.querySelector(".ws-run-choose") === null &&
  [...dialogEl.querySelectorAll(".ws-run-panel__row")].some((row) => row.textContent.includes("typed.md")));

dialogEl.querySelector(".ws-run-panel__input-browse").click();
await flush();
const inputDialog = dialogEl.querySelector(".ws-run-input");
check("the input row's Browse opens Choose Input", inputDialog !== null);
check(
  "Choose Input uses the form modal skin with Cancel first",
  inputDialog?.classList.contains("modal-dialog--form") === true &&
    dialogButtons(inputDialog).join(",") === "Cancel,Choose",
);
const inputPath = inputDialog.querySelector("input");
inputPath.value = `${ROOT}\\input.md`;
inputPath.dispatchEvent(new window.Event("input", { bubbles: true }));
inputDialog.querySelector(".modal-actions button:last-child").click();
await flush();
check(
  "Choose Input fills the input row's field",
  dialogEl.querySelector(".ws-run-input") === null &&
    [...dialogEl.querySelectorAll(".ws-run-panel__row")]
      .find((row) => row.querySelector(".ws-run-panel__row-label")?.textContent === "input")
      ?.querySelector("input")?.value === `${ROOT}\\input.md`,
);

brokenEl.querySelector(".ws-run-panel__choose").click();
await flush();
const errorChoose = brokenEl.querySelector(".ws-run-choose");
check("the error state's Choose Prompt link opens the same dialog", errorChoose !== null);
errorChoose.querySelector(".modal-actions button:first-child").click();
await flush();
check("Cancel dismisses Choose Prompt", brokenEl.querySelector(".ws-run-choose") === null);

// --- The stylesheet: Cursor Settings rows, states, toolbar, and footer ------

const resolve = await resolver();
const runCss = await readUi("src/parts/run/run-panel.css");
const runRules = rulesOf(runCss);
const css = (selector, property, at = null) => resolve(valueIn(runRules, selector, property, at));
const px = (selector, property) => css(selector, property);
const MUTED = "color-mix(in srgb, #f0f0f0 4%, transparent)";

check("the rows form a 4% tinted card", css(".ws-run-panel__rows", "background") === MUTED);
check("the card has a 12px radius", css(".ws-run-panel__rows", "border-radius") === "12px");
check("a row pads 12px", px(".ws-run-panel__row", "padding") === "12px");
check("a row's gap is 20px", px(".ws-run-panel__row", "gap") === "20px");
check(
  "an inset divider at 4% sits above every row after the first",
  css(".ws-run-panel__row + .ws-run-panel__row::before", "background") === MUTED &&
    css(".ws-run-panel__row + .ws-run-panel__row::before", "left") === "12px" &&
    css(".ws-run-panel__row + .ws-run-panel__row::before", "right") === "12px" &&
    css(".ws-run-panel__row + .ws-run-panel__row::before", "height") === "1px",
);
check("a row's label is 13px", px(".ws-run-panel__row-label", "font-size") === "13px");
check("a row's label is the primary text", css(".ws-run-panel__row-label", "color") === "#f0f0f0");
check("a description is 13px", px(".ws-run-panel__row-description", "font-size") === "13px");
check(
  "a description is the secondary text",
  css(".ws-run-panel__row-description", "color") === "color-mix(in srgb, #f0f0f0 74%, transparent)",
);
// The container is the content column, which is never the scroller, so its
// width excludes no scrollbar gutter: a panel 500 to 509px wide stays side by
// side. It is not the panel element, whose containment would make it the
// containing block for the dialogs' fixed overlay.
check(
  "the content column is the width container for the stacked layout",
  css(".ws-run-panel__content", "container-type") === "inline-size",
);
check(
  "the scrolling body is not the width container",
  css(".ws-run-panel__body", "container-type") === undefined &&
    css(".ws-run-panel__body", "overflow") === "auto",
);
check(
  "the panel element is not a container, so its dialogs' fixed overlay still covers the window",
  css(".ws-run-panel", "container-type") === undefined,
);
check(
  "rows stack under 500px: the row turns into a column",
  css(".ws-run-panel__row", "flex-direction", "width < 500px") === "column" &&
    css(".ws-run-panel__row-controls", "flex", "width < 500px") === "none",
);
check(
  "rows stay side by side at 500px and wider",
  css(".ws-run-panel__row", "flex-direction") === undefined,
);
check("the toolbar is a 35px strip", css(".ws-run-panel__toolbar", "height") === "35px");
check("the toolbar pads 8px", css(".ws-run-panel__toolbar", "padding") === "0 8px");
check("the path field is 24px tall", css(".ws-run-panel__prompt-path", "height") === "24px");
check("the path field text is 12px", css(".ws-run-panel__prompt-path", "font-size") === "12px");
check("the path field has a 2px radius", css(".ws-run-panel__prompt-path", "border-radius") === "2px");
check("the path field fill is #F0F0F00A", css(".ws-run-panel__prompt-path", "background") === "#f0f0f00a");
check(
  "the path field border is 1px #F0F0F013",
  css(".ws-run-panel__prompt-path", "border") === "1px solid #f0f0f013",
);
check(
  "the Prompt label is for screen readers only",
  css(".ws-run-panel__prompt-label", "position") === "absolute" &&
    css(".ws-run-panel__prompt-label", "clip-path") === "inset(50%)" &&
    css(".ws-run-panel__prompt-label", "width") === "1px",
);
check("a state line is 22px tall", css(".ws-run-panel__message", "line-height") === "22px");
check("a state line pads 20px on the left", css(".ws-run-panel__message", "padding-left") === "20px");
check(
  "an inline link is underlined, in the link color",
  css(".ws-run-panel__link", "text-decoration") === "underline" &&
    css(".ws-run-panel__link", "color") === "#81a1c1" &&
    css(".ws-run-panel__link", "display") === "inline",
);
check(
  "the drag-over overlay is #F0F0F011 and lets the drop through",
  css(".ws-run-panel--drag-over::after", "background") === "#f0f0f011" &&
    css(".ws-run-panel--drag-over::after", "pointer-events") === "none",
);
check("the footer pads 10px", css(".ws-run-panel__footer", "padding") === "10px");
check("the footer's gap is 8px", css(".ws-run-panel__footer", "gap") === "8px");
check("the footer is right-aligned", css(".ws-run-panel__footer", "justify-content") === "flex-end");
check(
  "the footer's top border is 8%",
  css(".ws-run-panel__footer", "border-top") === "1px solid color-mix(in srgb, #f0f0f0 8%, transparent)",
);
check(
  "the stylesheet declares no raw color or length",
  runRules.every((rule) => !/#[0-9a-f]{3,8}\b|\b\d*\.?\d+(px|rem|em)\b/i.test(rule.body)),
);

// --- Reduced motion: the shimmer degrades to a static muted title ------------

// jsdom applies no stylesheets, so the reduced-motion contract is checked
// against the stylesheet itself: under prefers-reduced-motion the
// animation, gradient, clip, and transparent fill all come off.
const shimmerCss = await readFile(
  fileURLToPath(import.meta.resolve("@workshop/look/shimmer.css")),
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
