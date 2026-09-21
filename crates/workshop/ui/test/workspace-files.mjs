// Unit test for the workspace-file actions (plan steps 10 and 11:
// src/parts/workspace-files/workspace-files.contribution.ts over
// src/services/workspace-file-client.ts). Bundles the contribution with
// esbuild - "@tauri-apps/plugin-dialog" and "@tauri-apps/api/event"
// aliased to the recording stubs in test/helpers - and drives the Open
// Workspace from File..., Save Workspace As..., and Duplicate
// Workspace... commands through the shared registries against jsdom
// with a scripted fetch. Covers: the catalog wiring (the stub rows' ids
// and labels, now wired: File > 2_open and 3_workspace, in the palette,
// no always-false precondition, Duplicate gaining its ellipsis); a
// cancelled picker performing no fetch; a successful open posting the
// picked path to /workspace/file/open, firing the
// promptforge:workspace-changed invalidation, emitting the
// promptforge:workspace-opened Tauri event with the path, and recording
// the path in the recent-files store, with nothing painted on the status
// bar; a run with a path argument (Open Recent, Ctrl+P; TWF-003) posting
// that path without ever reaching the picker, a non-string or absent
// argument still reaching it; a shell that rejects the emit leaving the open committed (the
// recent recorded, the invalidation fired, a console.warn and no
// unhandled rejection); a server refusal painting the error on the
// status bar while emitting nothing, recording nothing, and
// invalidating nothing; Save As and Duplicate seeding the save picker
// with "<current name>.pfwork", appending .pfwork to a bare name exactly
// once (never doubling an existing extension, any case), posting to
// save_as or duplicate, then invalidating, emitting, and recording the
// new path, with a cancel posting nothing and a refusal painting the
// error; and the client's typed parse of the wire shape (snake_case
// window_state) with a malformed answer refused. The UI state the switch
// moves (plan step 13) is covered by test/workspace-switch.mjs; here a
// minimal dock is bound through initZones so the actions' state apply and
// snapshot have something to run against, and the UI-state adapter stays
// the empty default.
// Run: node --test test/workspace-files.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/parts/workspace-files/workspace-files.contribution.ts";
      export { register } from "./src/parts/workspace-files/index.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus } from "./src/services/menu-registry.ts";
      export { RECENT_FILES_STORE, RecentFilesStore } from "./src/services/recent-files-store.ts";
      export { registerService } from "./src/services/service-registry.ts";
      export { currentWorkspaceFile, putWindowState } from "./src/services/workspace-file-client.ts";
      export { STATUS_BAR } from "./src/parts/status/status-bar.ts";
      export { initZones } from "./src/parts/layout/zones.ts";
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
    "@tauri-apps/api/event": path.join(uiDir, "helpers", "tauri-event-stub.mjs"),
  },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
// Events the bundle dispatches on the jsdom window must be jsdom-realm
// instances: jsdom's dispatchEvent rejects Node's Event.
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;

// The contribution registers at module scope; a malformed descriptor
// reports through console.error, so spy on it across the bundle import.
const consoleErrors = [];
const realConsoleError = console.error;
console.error = (...args) => {
  consoleErrors.push(args.join(" "));
};

const bundlePath = path.join(os.tmpdir(), "promptforge-workspace-files-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  register,
  Commands,
  Menus,
  RECENT_FILES_STORE,
  RecentFilesStore,
  registerService,
  currentWorkspaceFile,
  putWindowState,
  STATUS_BAR,
  initZones,
} = await import(pathToFileURL(bundlePath).href);
console.error = realConsoleError;

// A dock good enough for the zone registry: the default-layout fallback
// after an Open clears it and opens the two anchors; a Save As snapshots
// it. Nothing here asserts on the layout.
{
  const panels = new Map();
  const dock = {
    groups: [],
    onDidMovePanel: () => ({ dispose() {} }),
    onDidLayoutChange: () => ({ dispose() {} }),
    onDidRemovePanel: () => ({ dispose() {} }),
    onWillMutateLayout: () => ({ dispose() {} }),
    onDidMutateLayout: () => ({ dispose() {} }),
    getPanel: (id) => panels.get(id),
    getGroup: (id) => dock.groups.find((group) => group.id === id),
    addPanel: (options) => {
      const group = { id: `g-${options.id}`, api: { setSize() {}, isVisible: true, setVisible() {} } };
      dock.groups.push(group);
      const panel = { id: options.id, params: options.params, group, api: { setActive() {} } };
      panels.set(options.id, panel);
      return panel;
    },
    clear: () => {
      panels.clear();
      dock.groups.length = 0;
    },
    fromJSON: () => {},
    toJSON: () => ({ grid: { root: { type: "leaf", data: { views: [], id: "1" }, size: 1 }, width: 1, height: 1, orientation: "HORIZONTAL" }, panels: {} }),
  };
  initZones(dock);
}

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets an async action chain (dialog -> fetch -> json -> emit) settle.
async function flush() {
  for (let i = 0; i < 8; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- Shared fakes ------------------------------------------------------------

const statusMessages = [];
registerService(STATUS_BAR, () => ({
  showLocal: (label, severity) => statusMessages.push({ label, severity }),
}));
const recentStore = new RecentFilesStore(null);
registerService(RECENT_FILES_STORE, () => recentStore);

// Scripted server: every fetch is logged. A case that makes one kind of
// request sets nextAnswer, repeated for every fetch; a case that chains
// requests (Save As and Duplicate GET current before they POST) queues
// them in answerQueue, consumed in order before nextAnswer is consulted.
const fetches = [];
let nextAnswer = null;
const answerQueue = [];
globalThis.fetch = async (url, init) => {
  fetches.push({ url, method: init?.method ?? "GET", body: init?.body === undefined ? null : JSON.parse(init.body) });
  const answer = answerQueue.length > 0 ? answerQueue.shift() : nextAnswer;
  if (answer === null) {
    throw new Error(`unexpected fetch in the workspace-files test: ${url}`);
  }
  return { ok: answer.status < 400, status: answer.status, json: async () => answer.body };
};

/** The POST fetches made since the given count; the switch routes are all POSTs. */
function postsSince(count) {
  return fetches.slice(count).filter((f) => f.method === "POST");
}

let workspaceChanges = 0;
window.addEventListener("promptforge:workspace-changed", () => {
  workspaceChanges += 1;
});

window.__TAURI_INTERNALS__ = {};
window.__TAURI_DIALOG__ = { calls: [], answer: null };
window.__TAURI_EVENTS__ = { emitted: [], fail: false };

const OPENED = {
  path: "C:\\work\\Alpha.pfwork",
  name: "Alpha",
  grants: [{ path: "C:\\work\\src", exists: true }],
  window_state: { width: 1280, height: 800, x: 10, y: 20, maximized: false },
};

// --- Catalog wiring ----------------------------------------------------------

check("the contribution registers without a malformed descriptor", consoleErrors.length === 0);

{
  const command = Commands.lookup("workbench.action.openWorkspace");
  check("Open Workspace keeps the stub row's command id", command !== undefined);
  check("Open Workspace keeps the stub row's label", command?.title === "Open Workspace from File...");
  check("Open Workspace is no longer always disabled", command?.precondition !== "false");
  const row = Menus.getMenuItems("menubar/file").find((r) => r.command === "workbench.action.openWorkspace");
  check("Open Workspace sits in File > 2_open at the stub's position", row?.group === "2_open" && row?.order === 3);
  check(
    "Open Workspace reaches the command palette",
    Menus.getMenuItems("commandPalette").some((r) => r.command === "workbench.action.openWorkspace"),
  );
  const activation = register();
  check("the feature barrel's register() answers a disposable", typeof activation?.dispose === "function");
}

{
  const fileRows = Menus.getMenuItems("menubar/file");
  const saveAs = Commands.lookup("workbench.action.saveWorkspaceAs");
  check("Save Workspace As keeps the stub row's command id", saveAs !== undefined);
  check("Save Workspace As keeps the stub row's label", saveAs?.title === "Save Workspace As...");
  check("Save Workspace As is no longer always disabled", saveAs?.precondition !== "false");
  const saveAsRow = fileRows.find((r) => r.command === "workbench.action.saveWorkspaceAs");
  check("Save Workspace As sits in File > 3_workspace at the stub's position", saveAsRow?.group === "3_workspace" && saveAsRow?.order === 2);

  const duplicate = Commands.lookup("workbench.action.duplicateWorkspace");
  check("Duplicate Workspace keeps the stub row's command id", duplicate !== undefined);
  check("Duplicate Workspace gains its ellipsis", duplicate?.title === "Duplicate Workspace...");
  check("Duplicate Workspace is no longer always disabled", duplicate?.precondition !== "false");
  const duplicateRow = fileRows.find((r) => r.command === "workbench.action.duplicateWorkspace");
  check("Duplicate Workspace sits in File > 3_workspace at the stub's position", duplicateRow?.group === "3_workspace" && duplicateRow?.order === 3);

  const palette = Menus.getMenuItems("commandPalette").map((r) => r.command);
  check("Save Workspace As reaches the command palette", palette.includes("workbench.action.saveWorkspaceAs"));
  check("Duplicate Workspace reaches the command palette", palette.includes("workbench.action.duplicateWorkspace"));
}

// --- A cancelled picker performs no fetch -------------------------------------

{
  window.__TAURI_DIALOG__.answer = null;
  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  const pick = window.__TAURI_DIALOG__.calls.at(-1);
  check("Open Workspace opens the native file picker", pick?.kind === "open" && pick?.directory !== true);
  check(
    "the picker filters to .pfwork files",
    Array.isArray(pick?.filters) &&
      pick.filters.some((f) => f.name === "PromptForge Workspace" && f.extensions.join(",") === "pfwork"),
  );
  check("a cancelled picker performs no fetch", fetches.length === 0);
  check("a cancelled picker emits nothing", window.__TAURI_EVENTS__.emitted.length === 0);
  check("a cancelled picker records nothing", recentStore.list.length === 0);
}

// --- Success: post, invalidate, emit, record ----------------------------------

{
  window.__TAURI_DIALOG__.answer = OPENED.path;
  nextAnswer = { status: 200, body: OPENED };
  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  const post = fetches.at(-1);
  check("a picked file is posted to /workspace/file/open", post?.url === "/workspace/file/open" && post?.method === "POST");
  check("the post sends the picked path", post?.body?.path === OPENED.path);
  check("a successful open fires one workspace-changed invalidation", workspaceChanges === 1);
  const emitted = window.__TAURI_EVENTS__.emitted;
  check(
    "a successful open emits promptforge:workspace-opened with the path",
    emitted.length === 1 &&
      emitted[0].event === "promptforge:workspace-opened" &&
      emitted[0].payload?.path === OPENED.path,
  );
  check("a successful open records the path as recent", recentStore.list[0] === OPENED.path);
  check("a successful open paints nothing on the status bar", statusMessages.length === 0);
}

// --- A path argument skips the picker (Open Recent, Ctrl+P) ------------------

{
  const picksBefore = window.__TAURI_DIALOG__.calls.length;
  const changesBefore = workspaceChanges;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const statusBefore = statusMessages.length;
  const RECENT = "C:\\work\\Recent.pfwork";
  // A picker answer that must never be posted: a run with a path argument
  // never reaches the dialog.
  window.__TAURI_DIALOG__.answer = "C:\\work\\Wrong.pfwork";
  nextAnswer = { status: 200, body: { ...OPENED, path: RECENT, name: "Recent" } };
  await Commands.execute("workbench.action.openWorkspace", RECENT);
  await flush();
  check("a run with a path argument never opens the picker", window.__TAURI_DIALOG__.calls.length === picksBefore);
  const post = fetches.at(-1);
  check(
    "a run with a path argument posts that path to /workspace/file/open",
    post?.url === "/workspace/file/open" && post?.method === "POST" && post?.body?.path === RECENT,
  );
  check("a run with a path argument fires one workspace-changed invalidation", workspaceChanges === changesBefore + 1);
  check(
    "a run with a path argument emits promptforge:workspace-opened with that path",
    window.__TAURI_EVENTS__.emitted.length === emittedBefore + 1 && window.__TAURI_EVENTS__.emitted.at(-1).payload?.path === RECENT,
  );
  check("a run with a path argument records that path as recent", recentStore.list[0] === RECENT);
  check("a run with a path argument paints nothing on the status bar", statusMessages.length === statusBefore);

  // A non-string argument is not a path: the picker flow runs as before.
  const fetchesBefore = fetches.length;
  window.__TAURI_DIALOG__.answer = null;
  await Commands.execute("workbench.action.openWorkspace", 42);
  await flush();
  check("a run with a non-string argument still reaches the picker", window.__TAURI_DIALOG__.calls.length === picksBefore + 1);
  check("a cancelled picker after a non-string argument posts nothing", postsSince(fetchesBefore).length === 0);

  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  check("a run without an argument still reaches the picker", window.__TAURI_DIALOG__.calls.length === picksBefore + 2);
}

// --- A path argument whose open is refused paints the error -----------------------

{
  const picksBefore = window.__TAURI_DIALOG__.calls.length;
  const changesBefore = workspaceChanges;
  const recentBefore = recentStore.list.length;
  const GONE = "C:\\work\\Gone.pfwork";
  nextAnswer = { status: 404, body: { error: { code: "not_found", message: "workspace file not found: C:\\work\\Gone.pfwork" } } };
  await Commands.execute("workbench.action.openWorkspace", GONE);
  await flush();
  check("a refused open by path never opens the picker", window.__TAURI_DIALOG__.calls.length === picksBefore);
  check(
    "a refused open by path paints the server's message naming the path",
    statusMessages.at(-1)?.severity === "error" && statusMessages.at(-1)?.label.includes("Gone.pfwork"),
  );
  check("a refused open by path records nothing", recentStore.list.length === recentBefore);
  check("a refused open by path invalidates nothing", workspaceChanges === changesBefore);
}

// --- A rejected emit never undoes the open ------------------------------------

{
  const changesBefore = workspaceChanges;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const statusBefore = statusMessages.length;
  const warnings = [];
  const realConsoleWarn = console.warn;
  console.warn = (...args) => {
    warnings.push(args.join(" "));
  };
  const rejections = [];
  const onRejection = (reason) => rejections.push(reason);
  process.on("unhandledRejection", onRejection);
  const GAMMA = "C:\\work\\Gamma.pfwork";
  window.__TAURI_DIALOG__.answer = GAMMA;
  window.__TAURI_EVENTS__.fail = true;
  nextAnswer = { status: 200, body: { ...OPENED, path: GAMMA, name: "Gamma" } };
  try {
    await Commands.execute("workbench.action.openWorkspace");
    await flush();
  } finally {
    window.__TAURI_EVENTS__.fail = false;
    console.warn = realConsoleWarn;
    process.off("unhandledRejection", onRejection);
  }
  check("a rejected emit still posts the open", fetches.at(-1)?.body?.path === GAMMA);
  check("a rejected emit still fires the workspace-changed invalidation", workspaceChanges === changesBefore + 1);
  check("a rejected emit still records the path as recent", recentStore.list[0] === GAMMA);
  check("a rejected emit records no event", window.__TAURI_EVENTS__.emitted.length === emittedBefore);
  check(
    "a rejected emit is logged as a warning naming the event",
    warnings.length === 1 && warnings[0].includes("promptforge:workspace-opened") && warnings[0].includes("not allowed"),
  );
  check("a rejected emit paints nothing on the status bar", statusMessages.length === statusBefore);
  check("a rejected emit escapes as no unhandled rejection", rejections.length === 0);
}

// --- A server refusal: error shown, nothing emitted or recorded ---------------

{
  const changesBefore = workspaceChanges;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const recentBefore = recentStore.list.length;
  window.__TAURI_DIALOG__.answer = "C:\\elsewhere\\notes.txt";
  nextAnswer = {
    status: 400,
    body: { error: { code: "refused", message: "not a PromptForge workspace: format 'sqlite' where 'promptforge-workspace' required" } },
  };
  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  check(
    "a refused open paints the server's message on the status bar as error",
    statusMessages.some((m) => m.severity === "error" && m.label.includes("not a PromptForge workspace")),
  );
  check("a refused open emits nothing", window.__TAURI_EVENTS__.emitted.length === emittedBefore);
  check("a refused open records nothing", recentStore.list.length === recentBefore);
  check("a refused open invalidates nothing", workspaceChanges === changesBefore);
}

// --- A transport failure is reported the same way ------------------------------

{
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  window.__TAURI_DIALOG__.answer = "C:\\work\\Beta.pfwork";
  nextAnswer = null;
  await Commands.execute("workbench.action.openWorkspace");
  await flush();
  check(
    "a transport failure paints an error, never an unhandled rejection",
    statusMessages.at(-1)?.severity === "error" && statusMessages.at(-1)?.label.includes("Beta.pfwork"),
  );
  check("a transport failure emits nothing", window.__TAURI_EVENTS__.emitted.length === emittedBefore);
}

// --- Save As: cancel posts nothing --------------------------------------------

{
  const fetchesBefore = fetches.length;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const recentBefore = recentStore.list.length;
  const changesBefore = workspaceChanges;
  window.__TAURI_DIALOG__.answer = null;
  answerQueue.push({ status: 200, body: OPENED });
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  const pick = window.__TAURI_DIALOG__.calls.at(-1);
  check("Save Workspace As opens the native save picker", pick?.kind === "save");
  check("the save picker is seeded with the current name plus .pfwork", pick?.defaultPath === "Alpha.pfwork");
  check(
    "the save picker filters to .pfwork files",
    Array.isArray(pick?.filters) &&
      pick.filters.some((f) => f.name === "PromptForge Workspace" && f.extensions.join(",") === "pfwork"),
  );
  check("a cancelled save picker posts nothing", postsSince(fetchesBefore).length === 0);
  check("a cancelled save picker emits nothing", window.__TAURI_EVENTS__.emitted.length === emittedBefore);
  check("a cancelled save picker records nothing", recentStore.list.length === recentBefore);
  check("a cancelled save picker invalidates nothing", workspaceChanges === changesBefore);
}

// --- Save As: a bare name gains .pfwork once and switches ------------------------

{
  const fetchesBefore = fetches.length;
  const changesBefore = workspaceChanges;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const statusBefore = statusMessages.length;
  const BARE = "C:\\work\\Beta";
  const SAVED = `${BARE}.pfwork`;
  window.__TAURI_DIALOG__.answer = BARE;
  answerQueue.push({ status: 200, body: OPENED }, { status: 200, body: { ...OPENED, path: SAVED, name: "Beta" } });
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  const posts = postsSince(fetchesBefore);
  check("a picked name is posted once to /workspace/file/save_as", posts.length === 1 && posts[0].url === "/workspace/file/save_as");
  check("a bare name is posted with .pfwork appended", posts[0]?.body?.path === SAVED);
  check("a successful save-as fires one workspace-changed invalidation", workspaceChanges === changesBefore + 1);
  const emitted = window.__TAURI_EVENTS__.emitted;
  check(
    "a successful save-as emits promptforge:workspace-opened with the new path",
    emitted.length === emittedBefore + 1 && emitted.at(-1).event === "promptforge:workspace-opened" && emitted.at(-1).payload?.path === SAVED,
  );
  check("a successful save-as records the new path as recent", recentStore.list[0] === SAVED);
  check("a successful save-as paints nothing on the status bar", statusMessages.length === statusBefore);
}

// --- Save As: an existing extension is never doubled ------------------------------

{
  const fetchesBefore = fetches.length;
  const NAMED = "C:\\work\\Gamma.pfwork";
  window.__TAURI_DIALOG__.answer = NAMED;
  answerQueue.push({ status: 200, body: OPENED }, { status: 200, body: { ...OPENED, path: NAMED, name: "Gamma" } });
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  check("a name already ending in .pfwork is posted unchanged", postsSince(fetchesBefore)[0]?.body?.path === NAMED);

  const fetchesBeforeUpper = fetches.length;
  const UPPER = "C:\\work\\Delta.PFWORK";
  window.__TAURI_DIALOG__.answer = UPPER;
  answerQueue.push({ status: 200, body: OPENED }, { status: 200, body: { ...OPENED, path: UPPER, name: "Delta" } });
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  check("an upper-case .PFWORK extension is recognized and not doubled", postsSince(fetchesBeforeUpper)[0]?.body?.path === UPPER);
}

// --- Save As: a refusal paints the error and switches nothing ----------------------

{
  const changesBefore = workspaceChanges;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const recentBefore = recentStore.list.length;
  const TAKEN = "C:\\work\\Alpha.pfwork";
  window.__TAURI_DIALOG__.answer = TAKEN;
  answerQueue.push(
    { status: 200, body: OPENED },
    { status: 409, body: { error: { code: "conflict", message: "workspace file already exists: C:\\work\\Alpha.pfwork" } } },
  );
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  check(
    "a refused save-as paints the server's message on the status bar as error",
    statusMessages.at(-1)?.severity === "error" && statusMessages.at(-1)?.label.includes("already exists"),
  );
  check("a refused save-as emits nothing", window.__TAURI_EVENTS__.emitted.length === emittedBefore);
  check("a refused save-as records nothing", recentStore.list.length === recentBefore);
  check("a refused save-as invalidates nothing", workspaceChanges === changesBefore);
}

// --- Save As: an unreachable current still offers a default name --------------------

{
  const fetchesBefore = fetches.length;
  const statusBefore = statusMessages.length;
  window.__TAURI_DIALOG__.answer = null;
  answerQueue.push({ status: 500, body: { error: { code: "internal", message: "draining" } } });
  await Commands.execute("workbench.action.saveWorkspaceAs");
  await flush();
  const pick = window.__TAURI_DIALOG__.calls.at(-1);
  check("a failed current lookup still opens the save picker", pick?.kind === "save");
  check("a failed current lookup seeds the picker with Untitled.pfwork", pick?.defaultPath === "Untitled.pfwork");
  check("a failed current lookup paints nothing by itself", statusMessages.length === statusBefore && postsSince(fetchesBefore).length === 0);
}

// --- Duplicate: copies, switches, emits, records -----------------------------------

{
  const fetchesBefore = fetches.length;
  const changesBefore = workspaceChanges;
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const statusBefore = statusMessages.length;
  const COPY = "C:\\work\\Alpha copy.pfwork";
  window.__TAURI_DIALOG__.answer = "C:\\work\\Alpha copy";
  answerQueue.push({ status: 200, body: OPENED }, { status: 200, body: { ...OPENED, path: COPY, name: "Alpha copy" } });
  await Commands.execute("workbench.action.duplicateWorkspace");
  await flush();
  const pick = window.__TAURI_DIALOG__.calls.at(-1);
  check("Duplicate Workspace opens the native save picker seeded with the current name", pick?.kind === "save" && pick?.defaultPath === "Alpha.pfwork");
  const posts = postsSince(fetchesBefore);
  check("a picked name is posted once to /workspace/file/duplicate", posts.length === 1 && posts[0].url === "/workspace/file/duplicate");
  check("duplicate appends .pfwork to a bare name", posts[0]?.body?.path === COPY);
  check("a successful duplicate fires one workspace-changed invalidation", workspaceChanges === changesBefore + 1);
  const emitted = window.__TAURI_EVENTS__.emitted;
  check(
    "a successful duplicate emits promptforge:workspace-opened with the copy's path",
    emitted.length === emittedBefore + 1 && emitted.at(-1).payload?.path === COPY,
  );
  check("a successful duplicate records the copy's path as recent", recentStore.list[0] === COPY);
  check("a successful duplicate paints nothing on the status bar", statusMessages.length === statusBefore);
}

// --- Duplicate: a refusal (ephemeral workspace) paints the error ----------------------

{
  const emittedBefore = window.__TAURI_EVENTS__.emitted.length;
  const recentBefore = recentStore.list.length;
  window.__TAURI_DIALOG__.answer = "C:\\work\\Nothing.pfwork";
  answerQueue.push(
    { status: 200, body: { path: null, name: "Untitled", grants: [], window_state: null } },
    { status: 400, body: { error: { code: "refused", message: "no workspace file to duplicate: the workspace is ephemeral" } } },
  );
  await Commands.execute("workbench.action.duplicateWorkspace");
  await flush();
  check(
    "a refused duplicate paints the server's message on the status bar as error",
    statusMessages.at(-1)?.severity === "error" && statusMessages.at(-1)?.label.includes("ephemeral"),
  );
  check("a refused duplicate emits nothing", window.__TAURI_EVENTS__.emitted.length === emittedBefore);
  check("a refused duplicate records nothing", recentStore.list.length === recentBefore);
}

// --- The client's typed parse of the wire shape ----------------------------------

check("every queued server answer was consumed by the actions above", answerQueue.length === 0);
answerQueue.length = 0;

{
  nextAnswer = { status: 200, body: OPENED };
  const current = await currentWorkspaceFile();
  check("currentWorkspaceFile GETs /workspace/file/current", fetches.at(-1)?.url === "/workspace/file/current");
  check(
    "the client maps the wire's window_state onto windowState",
    current.path === OPENED.path &&
      current.name === "Alpha" &&
      current.grants.length === 1 &&
      current.grants[0].exists === true &&
      current.windowState?.width === 1280 &&
      current.windowState?.maximized === false,
  );

  nextAnswer = { status: 200, body: { path: null, name: "Untitled", grants: [], window_state: null } };
  const ephemeral = await currentWorkspaceFile();
  check("an ephemeral workspace parses with a null path and no geometry", ephemeral.path === null && ephemeral.windowState === null);

  nextAnswer = { status: 200, body: { name: 3 } };
  let shapeError = null;
  try {
    await currentWorkspaceFile();
  } catch (error) {
    shapeError = error;
  }
  check("a malformed answer is refused as an unexpected shape", shapeError !== null && /shape/.test(shapeError.message));

  nextAnswer = { status: 200, body: { saved: true } };
  const saved = await putWindowState({ width: 1, height: 2, x: 3, y: 4, maximized: true });
  const put = fetches.at(-1);
  check(
    "putWindowState PUTs the geometry to /workspace/file/window-state",
    put?.url === "/workspace/file/window-state" && put?.method === "PUT" && put?.body?.maximized === true,
  );
  check("putWindowState answers the server's saved flag", saved === true);
}

if (failures.length > 0) {
  console.error(`workspace-files: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workspace-files: all assertions passed");
