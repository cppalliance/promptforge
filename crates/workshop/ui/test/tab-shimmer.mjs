// The tab renderer's loading shimmer (src/parts/layout/panel-tab.ts) goes
// through @workshop/look's setShimmer, so a loading tab title shares the
// one sweep phase with every other shimmering element: the tab keeps no
// private epoch, class toggle, or animation delay of its own. The test
// bundles panel-tab.ts with `@workshop/look/shimmer` swapped for a recorder,
// so every call the tab makes is visible and the tab's own effect on the
// title is exactly what it does without the helper: nothing.
// Run: node test/tab-shimmer.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const recorder = {
  name: "shimmer-recorder",
  setup(build) {
    build.onResolve({ filter: /^@workshop\/look\/shimmer$/ }, () => ({
      path: "shimmer-recorder",
      namespace: "shimmer-recorder",
    }));
    build.onLoad({ filter: /.*/, namespace: "shimmer-recorder" }, () => ({
      contents: `
        export const calls = [];
        export function setShimmer(element, on) { calls.push({ element, on }); }
      `,
      loader: "js",
    }));
  },
};

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { PanelTab, setTabLoading } from "./src/parts/layout/panel-tab.ts";
      export { calls } from "@workshop/look/shimmer";
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
  plugins: [recorder],
  // The modules under test import their colocated CSS; the test drives only the JS.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", { url: "http://127.0.0.1:7910/" });
for (const name of ["window", "document", "Element", "HTMLElement", "Node", "MouseEvent", "KeyboardEvent", "Event"]) {
  globalThis[name] = dom.window[name];
}

const bundlePath = path.join(os.tmpdir(), "promptforge-tab-shimmer-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { PanelTab, setTabLoading, calls } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** A dockview tab-part init payload for one panel. */
function initParameters(id, tabLocation = "header") {
  return {
    api: {
      id,
      component: "test-panel",
      onDidTitleChange: () => ({ dispose() {} }),
    },
    title: "Run",
    tabLocation,
    params: {},
    containerApi: {},
    group: {},
  };
}

const contentOf = (tab) => tab.element.querySelector(".dv-default-tab-content");
const lastCall = () => calls[calls.length - 1];

// --- A header tab drives its title through setShimmer ---------------------------------

const tab = new PanelTab();
tab.init(initParameters("p1"));
const content = contentOf(tab);
check("mounting an idle tab asks setShimmer to keep the title still", lastCall()?.element === content && lastCall()?.on === false);

setTabLoading("p1", true);
check("loading turns the title shimmer on through setShimmer", lastCall()?.element === content && lastCall()?.on === true);
check("the tab adds no shimmer class of its own", !content.classList.contains("ws-shimmer-text"));
check("the tab sets no animation delay of its own", content.style.animationDelay === "" || content.style.animationDelay === undefined);

setTabLoading("p1", false);
check("ready turns the title shimmer off through setShimmer", lastCall()?.element === content && lastCall()?.on === false);

// --- A tab that mounts after loading started picks the state up ----------------------

setTabLoading("p2", true);
const late = new PanelTab();
late.init(initParameters("p2"));
check(
  "a tab mounted mid-load shimmers through setShimmer",
  lastCall()?.element === contentOf(late) && lastCall()?.on === true,
);

// --- An overflow copy under the same id shimmers too ----------------------------------

const overflow = new PanelTab();
overflow.init(initParameters("p2", "headerOverflow"));
check(
  "an overflow copy picks up the loading state through setShimmer",
  lastCall()?.element === contentOf(overflow) && lastCall()?.on === true,
);

tab.dispose();
late.dispose();
overflow.dispose();

if (failures.length > 0) {
  console.error(`tab-shimmer: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("tab-shimmer: all assertions passed");
