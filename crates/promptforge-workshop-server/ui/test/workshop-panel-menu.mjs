// Unit test for the Workshop tree's workspace management
// (src/ui/workshop/workshop-panel.ts): the root-row context menu, the
// missing-root rendering, and the Add Folder flows. Bundles the panel
// with esbuild - with "@tauri-apps/plugin-dialog" aliased to the scripted
// stub in test/helpers - and drives it against jsdom. Covers: a missing
// root renders with the strikethrough/danger class and a "missing" text
// label; right-clicking a root opens the shared dropdown with a danger
// Remove item that revokes the root, announces the change, and confirms
// on the status bar; a failed revoke paints a status-bar error and
// announces nothing; the header "+" button is a keyboard-focusable
// button; in the desktop app it opens the native folder picker and the
// picked path is granted (a cancelled pick grants nothing); in a plain
// browser it opens a dialog whose labeled path input gates the Add button
// until text is typed, Enter submits the typed path (and is inert while
// the field is empty), and a grant the server refuses paints a status-bar
// error.
// Run: node test/workshop-panel-menu.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

// One jsdom for the whole test: the lucide icon module serializes SVGs
// through the document at import time, so the globals must exist before
// the bundle loads.
const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.CustomEvent = window.CustomEvent;
globalThis.Event = window.Event;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.HTMLInputElement = window.HTMLInputElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `export { WorkshopTreePanel } from "./src/ui/workshop/workshop-panel.ts";`,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The panel's import graph pulls colocated CSS; the test drives only
  // the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/plugin-dialog": path.join(uiDir, "helpers", "tauri-dialog-stub.mjs"),
  },
});
const { WorkshopTreePanel } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets an async action chain (fetch -> json -> render) run to completion.
async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- Shared fetch mock -------------------------------------------------------

const LIVE_ROOT = "C:\\project";
const DEAD_ROOT = "C:\\gone";
const rootsListing = {
  path: null,
  entries: [
    { name: "project", path: LIVE_ROOT, kind: "directory", size: 0, modified_ms: 100, exists: true },
    { name: "gone", path: DEAD_ROOT, kind: "directory", size: 0, modified_ms: 0, exists: false },
  ],
};

const revokes = [];
const grants = [];
let revokeResponse = () => ({ ok: true, status: 200, json: async () => ({ revoked: "" }) });
let grantResponse = (path) => ({ ok: true, status: 200, json: async () => ({ granted: path }) });
globalThis.fetch = async (url, init) => {
  if (url === "/workspace/tree") {
    return { ok: true, status: 200, json: async () => rootsListing };
  }
  if (url === "/workspace/revoke") {
    revokes.push(JSON.parse(init.body).path);
    return revokeResponse();
  }
  if (url === "/workspace/grant") {
    const path = JSON.parse(init.body).path;
    grants.push(path);
    return grantResponse(path);
  }
  throw new Error(`unexpected fetch in the workshop-panel-menu test: ${url}`);
};

const statusMessages = [];
const statusBar = {
  showLocal: (label, severity) => statusMessages.push({ label, severity }),
};

let workspaceChanges = 0;
window.addEventListener("promptforge:workspace-changed", () => {
  workspaceChanges += 1;
});

function rightClick(target) {
  target.dispatchEvent(
    new window.MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 40, clientY: 40 }),
  );
}

function menu() {
  return window.document.querySelector(".workshop-dropdown");
}

function rowByName(panel, name) {
  return [...panel.element.querySelectorAll(".workshop-tree__row")].find(
    (row) => row.querySelector(".workshop-tree__name")?.textContent === name,
  );
}

// --- Missing roots render with the label, the class, and a working menu -----

const panelA = new WorkshopTreePanel(statusBar);
panelA.init();
window.document.body.appendChild(panelA.element);
await flush();

{
  const gone = rowByName(panelA, "gone");
  const live = rowByName(panelA, "project");
  check("both granted roots render as rows", !!gone && !!live);
  check(
    "a missing root carries the strikethrough/danger modifier class",
    gone?.classList.contains("workshop-tree__row--missing") === true,
  );
  check(
    'a missing root carries a "missing" text label beside its name',
    gone?.querySelector(".workshop-tree__missing")?.textContent === "missing",
  );
  check(
    "a live root carries neither the modifier class nor the label",
    live?.classList.contains("workshop-tree__row--missing") === false &&
      live?.querySelector(".workshop-tree__missing") === null,
  );
}

// --- Right-click on a root: Remove revokes, announces, and confirms ---------

{
  rightClick(rowByName(panelA, "project"));
  const items = [...(menu()?.querySelectorAll(".workshop-dropdown__item") ?? [])];
  check("right-clicking a root opens a one-item menu", items.length === 1);
  check(
    "the item is a danger-styled Remove from Workspace",
    items[0]?.textContent === "Remove from Workspace" &&
      items[0]?.classList.contains("workshop-dropdown__item--danger"),
  );
  items[0].click();
  await flush();
  check("Remove revokes the clicked root's path", revokes.join(",") === LIVE_ROOT);
  check("a successful revoke announces one workspace change", workspaceChanges === 1);
  check(
    "a successful revoke confirms on the status bar as info",
    statusMessages.some(
      (entry) => entry.severity === "info" && entry.label.includes("Removed") && entry.label.includes(LIVE_ROOT),
    ),
  );
  check("the menu closes after the action", menu() === null);
}

// --- A failed revoke paints the status bar and announces nothing ------------

{
  revokeResponse = () => ({
    ok: false,
    status: 404,
    json: async () => ({ error: { message: "path is not a granted root", code: "not_granted" } }),
  });
  const changesBefore = workspaceChanges;
  // The missing root's row still offers Remove: revoke is the cleanup
  // path for roots deleted from disk.
  rightClick(rowByName(panelA, "gone"));
  const items = [...(menu()?.querySelectorAll(".workshop-dropdown__item") ?? [])];
  check("a missing root's context menu still offers Remove", items[0]?.textContent === "Remove from Workspace");
  items[0].click();
  await flush();
  check(
    "a failed revoke paints the server message as a status-bar error",
    statusMessages.some(
      (entry) => entry.severity === "error" && entry.label.includes("path is not a granted root"),
    ),
  );
  check("a failed revoke announces no workspace change", workspaceChanges === changesBefore);
  revokeResponse = () => ({ ok: true, status: 200, json: async () => ({ revoked: "" }) });
}

// --- The header "+" button is keyboard-reachable -----------------------------

{
  const add = panelA.element.querySelector(".workshop-tree__add");
  check("the header renders an add-folder button", add instanceof window.HTMLButtonElement);
  check("the add button is not removed from the tab order", add?.tabIndex === 0);
  check("the add button has an accessible name", (add?.getAttribute("aria-label") ?? "").includes("Add Folder"));
  add.focus();
  check("the add button takes keyboard focus", window.document.activeElement === add);
}

panelA.dispose();

// --- Desktop app: the "+" opens the native picker; the picked path grants ---

{
  window.__TAURI_INTERNALS__ = {};
  window.__TAURI_DIALOG__ = { calls: [], answer: null };
  const panelB = new WorkshopTreePanel(statusBar);
  panelB.init();
  window.document.body.appendChild(panelB.element);
  await flush();

  panelB.element.querySelector(".workshop-tree__add").click();
  await flush();
  check(
    "the desktop add opens the native directory picker",
    window.__TAURI_DIALOG__.calls.length === 1 &&
      window.__TAURI_DIALOG__.calls[0].directory === true,
  );
  check("the desktop add opens no typed-path dialog", panelB.element.querySelector(".workspace-add-overlay") === null);

  // A cancelled pick resolves null: nothing grants.
  check("a cancelled pick grants nothing", grants.length === 0);

  const changesBefore = workspaceChanges;
  window.__TAURI_DIALOG__.answer = "C:\\picked";
  panelB.element.querySelector(".workshop-tree__add").click();
  await flush();
  check("a picked path is granted", grants.join(",") === "C:\\picked");
  check("a picked-path grant announces one workspace change", workspaceChanges === changesBefore + 1);
  check(
    "a picked-path grant confirms on the status bar as info",
    statusMessages.some(
      (entry) => entry.severity === "info" && entry.label.includes("Added") && entry.label.includes("C:\\picked"),
    ),
  );
  panelB.dispose();
}

// --- Plain browser: empty-space menu opens the dialog; input gates Add ------

{
  delete window.__TAURI_INTERNALS__;
  const panelC = new WorkshopTreePanel(statusBar);
  panelC.init();
  window.document.body.appendChild(panelC.element);
  await flush();

  // Right-click on empty panel space (the panel element itself).
  rightClick(panelC.element);
  const items = [...(menu()?.querySelectorAll(".workshop-dropdown__item") ?? [])];
  check(
    "right-clicking empty space offers Add Folder to Workspace...",
    items.length === 1 && items[0]?.textContent === "Add Folder to Workspace...",
  );
  items[0].click();

  const overlay = panelC.element.querySelector(".workspace-add-overlay");
  check("the browser add opens the path dialog", overlay !== null);
  const label = overlay?.querySelector('label[for="workspace-add-path"]');
  const input = overlay?.querySelector("input#workspace-add-path");
  check("the dialog's path input has an associated label", !!label && !!input);
  const addButton = [...(overlay?.querySelectorAll("button") ?? [])].find(
    (button) => button.textContent === "Add",
  );
  check("the Add button starts disabled while the input is empty", addButton?.disabled === true);
  addButton?.click();
  await flush();
  check("a disabled Add grants nothing", grants.length === 1);

  input.value = "C:\\typed";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  check("typing a path enables Add", addButton?.disabled === false);
  addButton.click();
  await flush();
  check("Add grants the typed path", grants.join(",") === "C:\\picked,C:\\typed");
  check("the dialog dismisses after Add", panelC.element.querySelector(".workspace-add-overlay") === null);
  panelC.dispose();
}

// --- Enter submits the dialog; a failed grant paints the status bar ---------

{
  const panelD = new WorkshopTreePanel(statusBar);
  panelD.init();
  window.document.body.appendChild(panelD.element);
  await flush();

  panelD.element.querySelector(".workshop-tree__add").click();
  const input = panelD.element.querySelector("input#workspace-add-path");
  const grantsBefore = grants.length;
  const pressEnter = (target) => {
    target.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
  };
  pressEnter(input);
  await flush();
  check("Enter with an empty field submits nothing", grants.length === grantsBefore);
  check(
    "Enter with an empty field keeps the dialog open",
    panelD.element.querySelector(".workspace-add-overlay") !== null,
  );

  input.value = "C:\\entered";
  input.dispatchEvent(new window.Event("input", { bubbles: true }));
  pressEnter(input);
  await flush();
  check("Enter with a typed path grants it", grants[grants.length - 1] === "C:\\entered");
  check("Enter dismisses the dialog", panelD.element.querySelector(".workspace-add-overlay") === null);

  // A grant the server refuses (a bad path never validated client-side)
  // paints the status bar and announces no workspace change.
  grantResponse = () => ({
    ok: false,
    status: 400,
    json: async () => ({ error: { message: "path is not absolute", code: "not_absolute" } }),
  });
  const changesBefore = workspaceChanges;
  panelD.element.querySelector(".workshop-tree__add").click();
  const retry = panelD.element.querySelector("input#workspace-add-path");
  retry.value = "relative\\path";
  retry.dispatchEvent(new window.Event("input", { bubbles: true }));
  pressEnter(retry);
  await flush();
  check(
    "a failed grant paints the server message as a status-bar error",
    statusMessages.some(
      (entry) => entry.severity === "error" && entry.label.includes("path is not absolute"),
    ),
  );
  check("a failed grant announces no workspace change", workspaceChanges === changesBefore);
  grantResponse = (path) => ({ ok: true, status: 200, json: async () => ({ granted: path }) });
  panelD.dispose();
}

if (failures.length > 0) {
  console.error(`workshop-panel-menu: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-panel-menu: all assertions passed");
