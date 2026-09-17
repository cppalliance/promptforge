// Unit test for the Workshop tree's restored expansion
// (src/ui/layout/workshop-panel.ts with src/services/tree-state-service.ts):
// after a relaunch the expanded set comes back from the workspace file
// but the listing cache is empty, so a folder that is expanded with no
// cached listing must fetch its listing on render rather than rendering
// open with no children. Bundles the panel with esbuild and drives it
// against jsdom with a mocked /workspace/tree. Covers: a folder in the
// initial expanded set with no cached listing is fetched on render and
// renders its children; the fetched listing lands in the cache; a nested
// expanded folder fetches once its parent renders; a collapsed folder is
// not fetched; a folder whose fetch fails paints an error row and leaves
// the rest of the tree standing; replaceExpanded on the service makes
// the panel re-render with the new set; a replaceExpanded that lands
// while a roots fetch from WORKSPACE_CHANGED_EVENT is still in flight
// (the Open Workspace sequence) renders each root once.
// Run: node test/workshop-panel-restore.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

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
    contents: `
      export { WorkshopTreePanel } from "./src/ui/layout/workshop-panel.ts";
      export { TreeStateService, TREE_STATE } from "./src/services/tree-state-service.ts";
      export { registerService } from "./src/services/service-registry.ts";
      export { WORKSPACE_CHANGED_EVENT } from "./src/ui/workspace/workspace-drops.ts";
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
const { WorkshopTreePanel, TreeStateService, TREE_STATE, registerService, WORKSPACE_CHANGED_EVENT } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- The mocked workspace tree -----------------------------------------------

const ROOT = "C:\\project";
const SRC = "C:\\project\\src";
const SRC_LIB = "C:\\project\\src\\lib";
const DOCS = "C:\\project\\docs";
const BROKEN = "C:\\project\\broken";

const dir = (name, p) => ({ name, path: p, kind: "directory", size: 0, modified_ms: 1, exists: true });
const file = (name, p) => ({ name, path: p, kind: "file", size: 3, modified_ms: 1, exists: true });

const listings = new Map([
  [null, { path: null, entries: [dir("project", ROOT)] }],
  [ROOT, { path: ROOT, entries: [dir("broken", BROKEN), dir("docs", DOCS), dir("src", SRC)] }],
  [SRC, { path: SRC, entries: [dir("lib", SRC_LIB), file("main.ts", `${SRC}\\main.ts`)] }],
  [SRC_LIB, { path: SRC_LIB, entries: [file("util.ts", `${SRC_LIB}\\util.ts`)] }],
  [DOCS, { path: DOCS, entries: [file("guide.md", `${DOCS}\\guide.md`)] }],
]);

const fetched = [];
// While set, every roots fetch (path null) parks on this promise so a test
// can start a second load before the first resolves.
let holdRoots = null;
globalThis.fetch = async (url) => {
  const parsed = new URL(url, "http://127.0.0.1:7910/");
  if (parsed.pathname !== "/workspace/tree") {
    throw new Error(`unexpected fetch in the workshop-panel-restore test: ${url}`);
  }
  const p = parsed.searchParams.get("path");
  fetched.push(p);
  if (p === null && holdRoots !== null) {
    await holdRoots;
  }
  if (p === BROKEN) {
    return {
      ok: false,
      status: 403,
      json: async () => ({ error: { message: "path is outside the workspace", code: "forbidden" } }),
    };
  }
  const listing = listings.get(p);
  if (listing === undefined) {
    throw new Error(`no mocked listing for ${p}`);
  }
  return { ok: true, status: 200, json: async () => listing };
};

function rowByPath(panel, p) {
  return [...panel.element.querySelectorAll(".ws-workshop-tree__row")].find((row) => row.title === p);
}
function childrenOf(row) {
  return row?.parentElement?.querySelector(".ws-workshop-tree__children") ?? null;
}
function childNames(row) {
  const children = childrenOf(row);
  return children === null
    ? []
    : [...children.children].map(
        (li) => li.querySelector(".ws-workshop-tree__name")?.textContent ?? li.textContent,
      );
}

// --- Restored expansion fetches listings on render ------------------------

// The state a relaunch produces: the workspace file says these folders
// were open; nothing has been fetched yet this session.
const storage = createFakeUiStorage({ workspace: { tree: { expanded: [ROOT, SRC, SRC_LIB, BROKEN] } } });
const state = new TreeStateService(storage.get("workspace", "tree"), (value) =>
  storage.set("workspace", "tree", value),
);
registerService(TREE_STATE, () => state);

const panel = new WorkshopTreePanel(null);
panel.init();
window.document.body.appendChild(panel.element);
await flush();

{
  const root = rowByPath(panel, ROOT);
  check("the granted root renders", root !== undefined);
  check("a restored root renders expanded", root?.getAttribute("aria-expanded") === "true");
  check("a restored root with no cached listing is fetched on render", fetched.includes(ROOT));
  check(
    "the restored root shows its children",
    childNames(root).join(",") === "broken,docs,src",
  );
  check("the fetched listing lands in the cache", state.listing(ROOT) !== undefined);
  check("the children list is visible", childrenOf(root)?.hidden === false);

  const src = rowByPath(panel, SRC);
  check("a nested restored folder fetches once its parent renders", fetched.includes(SRC));
  check("the nested folder renders expanded with its children", src?.getAttribute("aria-expanded") === "true" && childNames(src).join(",") === "lib,main.ts");
  check("a third level restores too", fetched.includes(SRC_LIB) && childNames(rowByPath(panel, SRC_LIB)).join(",") === "util.ts");

  const docs = rowByPath(panel, DOCS);
  check("a collapsed folder is not fetched", !fetched.includes(DOCS));
  check("a collapsed folder renders collapsed", docs?.getAttribute("aria-expanded") === "false" && childrenOf(docs)?.hidden === true);

  const broken = rowByPath(panel, BROKEN);
  check("a restored folder whose fetch fails was attempted", fetched.includes(BROKEN));
  const errorRow = childrenOf(broken)?.querySelector(".ws-workshop-tree__error");
  check("a failed restore paints an error row under the folder", errorRow?.textContent.includes("outside the workspace") === true);
  check("a failed restore leaves the folder's row enabled", broken?.disabled === false);
  check("a failed restore leaves the rest of the tree standing", childNames(root).length === 3 && childNames(src).length === 2);
  check("nothing is fetched twice", new Set(fetched).size === fetched.length);
  check("rendering a restored tree writes nothing back", storage.sets.length === 0);
}

// --- replaceExpanded re-renders the tree -------------------------------------

{
  const before = fetched.length;
  state.replaceExpanded([ROOT, DOCS]);
  await flush();
  const root = rowByPath(panel, ROOT);
  check("after replaceExpanded the root is still expanded", root?.getAttribute("aria-expanded") === "true");
  check("after replaceExpanded the old folders render collapsed", rowByPath(panel, SRC)?.getAttribute("aria-expanded") === "false");
  const docs = rowByPath(panel, DOCS);
  check("after replaceExpanded the new folder renders expanded", docs?.getAttribute("aria-expanded") === "true");
  check("the newly expanded folder is fetched", fetched.slice(before).includes(DOCS));
  check("the newly expanded folder shows its children", childNames(docs).join(",") === "guide.md");
  check("already cached listings are not refetched", !fetched.slice(before).includes(ROOT) && !fetched.slice(before).includes(SRC));
  check(
    "the tree renders one copy of the root",
    [...panel.element.querySelectorAll(".ws-workshop-tree__row")].filter((row) => row.title === ROOT).length === 1,
  );
}

// --- replaceExpanded during an in-flight roots fetch --------------------------

// The Open Workspace sequence: WORKSPACE_CHANGED_EVENT invalidates the
// roots and starts a fetch; replaceExpanded arrives a round-trip later
// while that fetch is unresolved and starts a second one (the cache is
// still empty). Only the later load may render, or every root doubles.
{
  const before = fetched.length;
  let release;
  holdRoots = new Promise((resolve) => {
    release = resolve;
  });
  window.dispatchEvent(new CustomEvent(WORKSPACE_CHANGED_EVENT));
  await flush();
  check("the workspace change starts a roots fetch", fetched.slice(before).filter((p) => p === null).length === 1);
  check("the roots are not cached while the fetch is in flight", state.listing("") === undefined);
  state.replaceExpanded([ROOT, SRC]);
  await flush();
  check("replaceExpanded during the fetch starts a second roots fetch", fetched.slice(before).filter((p) => p === null).length === 2);
  release();
  holdRoots = null;
  await flush();
  const roots = [...panel.element.querySelectorAll(".ws-workshop-tree__row")].filter((row) => row.title === ROOT);
  check("overlapping loads render one copy of each root", roots.length === 1);
  check("the roots listing is cached once both loads settle", state.listing("") !== undefined);
  check("the surviving render is expanded per the replaced set", roots[0]?.getAttribute("aria-expanded") === "true" && childNames(roots[0]).join(",") === "broken,docs,src");
  check("the replaced set's nested folder renders expanded", rowByPath(panel, SRC)?.getAttribute("aria-expanded") === "true");
  check("the replaced set's collapsed folder renders collapsed", rowByPath(panel, DOCS)?.getAttribute("aria-expanded") === "false");
  check("no error row appears from the dropped load", panel.element.querySelector(".ws-workshop-tree__error") === null);
}

panel.dispose();
state.dispose();

if (failures.length > 0) {
  console.error(`workshop-panel-restore: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-panel-restore: all assertions passed");
