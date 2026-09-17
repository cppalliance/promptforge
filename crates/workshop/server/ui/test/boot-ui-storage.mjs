// Boot test for the UI-state preload in the composition root (src/main.ts):
// the app fetches GET /user/state and GET /workspace/file/state before it
// resolves any service (the first getService call, recorded by the boot
// fixture's registry seam as a RESOLVE entry in the fetch log); with both
// buckets answering, every store holds its bucket's value (the four user
// stores and the zoom from ui-state.json, the layout, tree, and closed
// editors from the workspace file); a bucket that fails yields defaults
// for its stores only, with one warning; and a server that never answers
// still lets boot complete on defaults after the preload timeout with one
// warning per bucket. Stores are registry services, so the first
// resolution of any token bounds the first store construction from
// above, and the fixture's resolveService reads each one back by id.
// Every scenario also asserts that the booted workbench fetched the
// granted roots (GET /workspace/tree, path null) exactly once: the tree
// panel and the window title share one load through TreeStateService.
//
// bootWorkbench exits the process, so each scenario boots in its own child
// process; run without arguments this file drives them all and fails if
// any does. Run: node test/boot-ui-storage.mjs (after `npm run build`).
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { bootWorkbench } from "./helpers/boot.mjs";

const USER_STATE = "/user/state";
const WORKSPACE_STATE = "/workspace/file/state";
// The granted-roots listing; the fixture answers it empty. fetchTree
// sends the bare route for the roots (no ?path=), so an exact match is
// the `path=null` request.
const WORKSPACE_TREE = "/workspace/tree";
// main.ts preloads with a 3000 ms timeout; the hanging scenario allows the
// timer some slack either side.
const PRELOAD_TIMEOUT_MS = 3000;

// Counts the adapter's own console warnings across the whole boot; the
// bundle writes to the Node global console, which jsdom does not replace.
const uiStorageWarnings = [];
const realWarn = console.warn;
console.warn = (...args) => {
  const text = args.map(String).join(" ");
  if (text.startsWith("ui-storage:")) {
    uiStorageWarnings.push(text);
  } else {
    realWarn(...args);
  }
};

const isStateGet = (entry) =>
  entry.method === "GET" && (entry.url === USER_STATE || entry.url === WORKSPACE_STATE);
const stateGets = (fetchLog) => fetchLog.filter(isStateGet);
// The first service resolution of the boot; `url` carries the token id.
const firstResolve = (fetchLog) => fetchLog.find((entry) => entry.method === "RESOLVE");

// Both state GETs recorded once each, both before the first getService.
function checkOrdering(fetchLog, failures) {
  const gets = stateGets(fetchLog);
  const urls = gets.map((entry) => entry.url).sort();
  if (urls.join(",") !== [USER_STATE, WORKSPACE_STATE].join(",")) {
    failures.push(
      `boot must GET each state bucket exactly once, saw ${JSON.stringify(gets.map((e) => e.url))}`,
    );
  }
  const resolveIndex = fetchLog.findIndex((entry) => entry.method === "RESOLVE");
  if (resolveIndex === -1) {
    failures.push("boot never resolved a service; the registry seam is not recording");
    return;
  }
  const lastStateGet = fetchLog.reduce((last, entry, index) => (isStateGet(entry) ? index : last), -1);
  if (lastStateGet > resolveIndex) {
    failures.push(
      `both state GETs must precede the first getService (${fetchLog[resolveIndex].url}); log: ${JSON.stringify(fetchLog.map((e) => `${e.method} ${e.url}`))}`,
    );
  }
  checkRootsFetch(fetchLog, failures);
}

// The booted workbench (tree panel mounted, title rendered) fetched the
// roots once: the panel and the window title share TreeStateService's load.
function checkRootsFetch(fetchLog, failures) {
  const rootsGets = fetchLog.filter((entry) => entry.method === "GET" && entry.url === WORKSPACE_TREE);
  if (rootsGets.length !== 1) {
    failures.push(
      `boot must GET ${WORKSPACE_TREE} (the roots, path null) exactly once, saw ${rootsGets.length}; log: ${JSON.stringify(fetchLog.filter((e) => e.method !== "RESOLVE").map((e) => `${e.method} ${e.url}`))}`,
    );
  }
}

// --- Seeded bucket documents and what each store must read from them -----

// The user bucket (ui-state.json), every key set off its default.
const USER_DOC = {
  editor_settings: { wordWrap: true, renderWhitespace: true, renderControlCharacters: false, columnSelection: true },
  zoom: 1.3,
  recent_files: ["C:\\seed\\a.md", "C:\\seed\\b.md"],
  commands_history: ["workbench.action.zoomIn", "workbench.action.files.save"],
};
const DEFAULT_EDITOR_SETTINGS = { wordWrap: false, renderWhitespace: false, renderControlCharacters: true, columnSelection: false };

// The workspace bucket (.pfwork kv rows): a v3 layout envelope as
// buildLayoutEnvelope writes it (tree left, agent right) whose group ids
// are distinctive, so a restore is told from the default layout by the
// zone map alone; an expanded set; a closed stack.
const SEEDED_LAYOUT = {
  version: 3,
  zones: { left: "seeded-left", right: "seeded-right" },
  overrides: {},
  layout: {
    grid: {
      root: {
        type: "branch",
        data: [
          { type: "leaf", data: { views: ["tree"], activeView: "tree", id: "seeded-left" }, size: 100 },
          { type: "leaf", data: { views: ["agent"], activeView: "agent", id: "seeded-right" }, size: 100 },
        ],
        size: 100,
      },
      width: 100,
      height: 100,
      orientation: "HORIZONTAL",
    },
    panels: {
      tree: { id: "tree", contentComponent: "tree", tabComponent: "permanent", title: "Workshop" },
      agent: { id: "agent", contentComponent: "agent", tabComponent: "agent-tab", title: "Agent Session" },
    },
    activeGroup: "seeded-right",
  },
};
const WORKSPACE_DOC = {
  layout: SEEDED_LAYOUT,
  tree: { expanded: ["C:\\seed", "C:\\seed\\src"] },
  closed_editors: { paths: ["C:\\seed\\old.md", "C:\\seed\\older.md"] },
};

// Asserts the user-bucket stores against `doc`, or the defaults when
// `doc` is null. Zoom applies to the document in browser mode (no Tauri
// shell in the fixture): a restore sets the root zoom and pins the body;
// the default path never touches either.
function checkUserStores({ resolveService, document }, doc, failures) {
  const expect = (what, actual, expected) => {
    if (!isDeepStrictEqual(actual, expected)) {
      failures.push(`${what}: expected ${JSON.stringify(expected)}, saw ${JSON.stringify(actual)}`);
    }
  };
  expect("editor settings", resolveService("workshop.editorSettings").settings, doc?.editor_settings ?? DEFAULT_EDITOR_SETTINGS);
  expect("recent files", [...resolveService("workshop.recentFiles").list], doc?.recent_files ?? []);
  expect("commands history", [...resolveService("workshop.commandsHistory").list], doc?.commands_history ?? []);
  const zoomApplied = document.body.style.position === "relative";
  expect("zoom restored", zoomApplied, doc !== null);
  if (doc !== null) {
    expect("zoom factor", document.documentElement.style.zoom, String(doc.zoom));
  }
}

// Asserts the workspace-bucket stores against `doc`, or the defaults
// when `doc` is null: the default layout's groups are Dockview-numbered,
// never the seeded ids.
function checkWorkspaceStores({ resolveService }, doc, failures) {
  const expect = (what, actual, expected) => {
    if (!isDeepStrictEqual(actual, expected)) {
      failures.push(`${what}: expected ${JSON.stringify(expected)}, saw ${JSON.stringify(actual)}`);
    }
  };
  const zones = resolveService("workshop.zoneState");
  const groups = [zones.groupFor("left"), zones.groupFor("right")];
  if (doc) {
    expect("layout restored through the zone map", groups, ["seeded-left", "seeded-right"]);
  } else if (groups.some((id) => typeof id !== "string" || id.startsWith("seeded-"))) {
    failures.push(`the default layout must anchor both zones under Dockview's own group ids, saw ${JSON.stringify(groups)}`);
  }
  expect("tree expansion", [...resolveService("workshop.treeState").expandedPaths], doc?.tree.expanded ?? []);
  expect("closed editors", resolveService("workshop.closedEditors").snapshot().paths, doc?.closed_editors.paths ?? []);
}

const scenarios = {
  // Both buckets answer: the two GETs are the first entries of the boot
  // log, ahead of every service resolution, nothing warns, and every
  // store holds its bucket's value.
  resolve: () =>
    bootWorkbench(
      "boot preloads both UI-state buckets before any store resolves and seeds every store",
      async (ctx) => {
        const { fetchLog, failures } = ctx;
        checkOrdering(fetchLog, failures);
        const firstTwo = fetchLog.slice(0, 2).map((entry) => `${entry.method} ${entry.url}`).sort();
        if (firstTwo.join(",") !== `GET ${USER_STATE},GET ${WORKSPACE_STATE}`) {
          failures.push(`the state GETs must be the first two fetches, saw ${JSON.stringify(firstTwo)}`);
        }
        if (uiStorageWarnings.length !== 0) {
          failures.push(`a clean preload must not warn, saw ${JSON.stringify(uiStorageWarnings)}`);
        }
        checkUserStores(ctx, USER_DOC, failures);
        checkWorkspaceStores(ctx, WORKSPACE_DOC, failures);
      },
      { uiState: { user: USER_DOC, workspace: WORKSPACE_DOC } },
    ),

  // The user bucket fails, the workspace bucket answers: the user stores
  // boot on defaults with one warning naming that GET, and the workspace
  // stores still hold the file's values.
  userFails: () =>
    bootWorkbench(
      "a failed user bucket yields defaults for the user stores only",
      async (ctx) => {
        const { fetchLog, failures } = ctx;
        checkOrdering(fetchLog, failures);
        if (uiStorageWarnings.length !== 1 || !uiStorageWarnings[0].includes(`GET ${USER_STATE}`)) {
          failures.push(`the failed bucket must warn exactly once, naming GET ${USER_STATE}; saw ${JSON.stringify(uiStorageWarnings)}`);
        }
        checkUserStores(ctx, null, failures);
        checkWorkspaceStores(ctx, WORKSPACE_DOC, failures);
      },
      { uiState: { user: "reject", workspace: WORKSPACE_DOC } },
    ),

  // The workspace bucket fails, the user bucket answers: the mirror image.
  workspaceFails: () =>
    bootWorkbench(
      "a failed workspace bucket yields defaults for the workspace stores only",
      async (ctx) => {
        const { fetchLog, failures } = ctx;
        checkOrdering(fetchLog, failures);
        if (uiStorageWarnings.length !== 1 || !uiStorageWarnings[0].includes(`GET ${WORKSPACE_STATE}`)) {
          failures.push(`the failed bucket must warn exactly once, naming GET ${WORKSPACE_STATE}; saw ${JSON.stringify(uiStorageWarnings)}`);
        }
        checkUserStores(ctx, USER_DOC, failures);
        checkWorkspaceStores(ctx, null, failures);
      },
      { uiState: { user: USER_DOC, workspace: "reject" } },
    ),

  // Both buckets hang: boot still completes (bootWorkbench itself fails
  // when the panels never mount), the first service resolves only after
  // the preload timeout rather than early, each bucket warns exactly
  // once, and every store holds its defaults.
  hang: () =>
    bootWorkbench(
      "boot completes on defaults when both UI-state buckets hang",
      async (ctx) => {
        const { fetchLog, failures } = ctx;
        checkOrdering(fetchLog, failures);
        checkUserStores(ctx, null, failures);
        checkWorkspaceStores(ctx, null, failures);
        const gets = stateGets(fetchLog);
        const resolve = firstResolve(fetchLog);
        if (gets.length === 2 && resolve) {
          const waited = resolve.at - Math.max(...gets.map((entry) => entry.at));
          if (waited < PRELOAD_TIMEOUT_MS - 200) {
            failures.push(
              `the first service resolved ${waited} ms after the state GETs; boot must wait out the ${PRELOAD_TIMEOUT_MS} ms preload timeout`,
            );
          }
          if (waited > PRELOAD_TIMEOUT_MS + 2000) {
            failures.push(
              `the first service resolved ${waited} ms after the state GETs; boot must not block past the timeout`,
            );
          }
        }
        const perBucket = [USER_STATE, WORKSPACE_STATE].map(
          (route) => uiStorageWarnings.filter((text) => text.includes(`GET ${route}`)).length,
        );
        if (perBucket.join(",") !== "1,1") {
          failures.push(
            `each hanging bucket must warn exactly once, saw ${JSON.stringify(uiStorageWarnings)}`,
          );
        }
      },
      { uiState: { user: "hang", workspace: "hang" } },
    ),
};

const scenarioFlag = process.argv.find((arg) => arg.startsWith("--scenario="));
if (scenarioFlag) {
  const scenario = scenarios[scenarioFlag.slice("--scenario=".length)];
  if (!scenario) {
    console.error(`unknown scenario ${scenarioFlag}; known: ${Object.keys(scenarios).join(", ")}`);
    process.exit(1);
  }
  await scenario();
} else {
  // Driver: one child per scenario, output forwarded, any failure fails
  // the file. Sequential so the two boots' timing assertions never share a
  // CPU with each other.
  const self = fileURLToPath(import.meta.url);
  let failed = 0;
  for (const name of Object.keys(scenarios)) {
    const code = await new Promise((resolve) => {
      const child = spawn(process.execPath, [self, `--scenario=${name}`], { stdio: "inherit" });
      child.on("exit", (exitCode) => resolve(exitCode ?? 1));
      child.on("error", () => resolve(1));
    });
    if (code !== 0) failed += 1;
  }
  process.exit(failed === 0 ? 0 : 1);
}
