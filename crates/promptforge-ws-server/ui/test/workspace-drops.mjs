// Unit test for the native workspace drop handler (src/workspace-drops.ts).
// Bundles the TS module with esbuild, imports it via a data URL, and drives
// it against jsdom. Covers: browser mode never installs the grant listener;
// in desktop mode a synthesized promptforge:file-drop event POSTs one grant
// per path with a paths-only JSON body; malformed details are ignored; a
// failed grant paints the status bar and does not stop the rest; the
// default action of file drags is suppressed (never navigating away) while
// in-page drags keep their default; and a file drop posts its File objects
// over the WebView2 bridge when one is present.
// Run: node test/workspace-drops.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "workspace-drops.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const code = bundle.outputFiles[0].text;
const { setupWorkspaceDrops } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets the async grant chain (fetch -> json -> next path) run to completion.
async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// Each scenario gets a fresh jsdom and fetch mock. `responder` maps a
// granted path to its fake Response; default is a plain 200.
function scenario({ desktop, responder }) {
  const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  if (desktop) {
    window.__PROMPTFORGE_DESKTOP__ = true;
  }
  const calls = [];
  const local = [];
  globalThis.window = window;
  globalThis.CustomEvent = window.CustomEvent;
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init, body: JSON.parse(init.body) });
    const respond =
      responder ??
      ((path) => ({ ok: true, status: 200, json: async () => ({ granted: path }) }));
    return respond(JSON.parse(init.body).path);
  };
  const statusBar = {
    showLocal: (label, severity) => local.push({ label, severity }),
  };
  setupWorkspaceDrops(statusBar);
  return { window, calls, local };
}

// --- Browser mode: no flag, no listener, no grants --------------------------

{
  const { window, calls } = scenario({ desktop: false });
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", {
      detail: { paths: ["C:\\Users\\Vinnie\\project"] },
    }),
  );
  await flush();
  check("browser mode never grants a dropped path", calls.length === 0);
}

// --- Desktop mode: a synthesized drop grants each path ----------------------

{
  const { window, calls, local } = scenario({ desktop: true });
  const paths = [
    "C:\\Users\\Vinnie\\My Documents\\project",
    "C:\\Users\\Vinnie\\café 中文.txt",
  ];
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", { detail: { paths } }),
  );
  await flush();
  check("one grant request per dropped path", calls.length === 2);
  check(
    "every grant posts to the workspace grant route",
    calls.every((call) => call.url === "/workspace/grant" && call.init.method === "POST"),
  );
  check(
    "grant bodies carry the paths and nothing else",
    calls.every(
      (call, index) =>
        Object.keys(call.body).join(",") === "path" && call.body.path === paths[index],
    ),
  );
  check(
    "grant bodies are JSON with a JSON content type",
    calls.every((call) => call.init.headers["Content-Type"] === "application/json"),
  );
  check("successful grants stay off the status bar", local.length === 0);
}

// --- Malformed events: nothing is granted -----------------------------------

{
  const { window, calls } = scenario({ desktop: true });
  window.dispatchEvent(new window.Event("promptforge:file-drop"));
  window.dispatchEvent(new window.CustomEvent("promptforge:file-drop", { detail: null }));
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", { detail: { paths: "C:\\x" } }),
  );
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", { detail: { paths: ["C:\\x", 42] } }),
  );
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", { detail: { paths: [] } }),
  );
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", { detail: { other: ["C:\\x"] } }),
  );
  await flush();
  check("malformed or empty drop details never grant", calls.length === 0);
}

// --- File-drag defaults are suppressed; in-page drags keep theirs ------------

// jsdom has no DragEvent; a plain Event with a scripted dataTransfer is
// enough for the handler's `types` probe and defaultPrevented flag.
function syntheticDrag(window, type, types) {
  const event = new window.Event(type, { cancelable: true, bubbles: true });
  Object.defineProperty(event, "dataTransfer", { value: { types } });
  return event;
}

for (const desktop of [false, true]) {
  const { window } = scenario({ desktop });
  const fileOver = syntheticDrag(window, "dragover", ["Files"]);
  window.dispatchEvent(fileOver);
  check(
    `file dragover default is prevented (desktop=${desktop})`,
    fileOver.defaultPrevented,
  );
  const fileDrop = syntheticDrag(window, "drop", ["Files"]);
  window.dispatchEvent(fileDrop);
  check(`file drop never navigates the page (desktop=${desktop})`, fileDrop.defaultPrevented);
  const tabDrag = syntheticDrag(window, "dragover", ["text/plain", "dockview/tab"]);
  window.dispatchEvent(tabDrag);
  check(
    `in-page drags keep their default action (desktop=${desktop})`,
    !tabDrag.defaultPrevented,
  );
  const bareDrop = syntheticDrag(window, "drop", undefined);
  window.dispatchEvent(bareDrop);
  check(
    `a drop without a dataTransfer is left alone (desktop=${desktop})`,
    !bareDrop.defaultPrevented,
  );
}

// --- The WebView2 bridge receives a drop's File objects ----------------------

// A drop carrying files posts them to the shell under the workspace-drop
// message; without the bridge (plain browser) the same drop is only
// default-suppressed. jsdom lacks DragEvent and File, so plain markers
// stand in for the File objects - the module hands them over untouched.
{
  const { window } = scenario({ desktop: true });
  const posted = [];
  window.chrome = {
    webview: {
      postMessageWithAdditionalObjects: (message, objects) =>
        posted.push({ message, objects }),
    },
  };
  const fileA = { name: "a.txt" };
  const fileB = { name: "b" };
  const drop = syntheticDrag(window, "drop", ["Files"]);
  // Array-like is all Array.from needs from the FileList stand-in.
  Object.defineProperty(drop.dataTransfer, "files", {
    value: { length: 2, 0: fileA, 1: fileB },
  });
  window.dispatchEvent(drop);
  check("a file drop posts one workspace-drop message", posted.length === 1);
  check(
    "the message carries the sentinel and every dropped File",
    posted.length === 1 &&
      posted[0].message === "workspace-drop" &&
      posted[0].objects.length === 2 &&
      posted[0].objects[0] === fileA &&
      posted[0].objects[1] === fileB,
  );

  const emptyDrop = syntheticDrag(window, "drop", ["Files"]);
  Object.defineProperty(emptyDrop.dataTransfer, "files", { value: { length: 0 } });
  window.dispatchEvent(emptyDrop);
  check("a file drag with no files posts nothing", posted.length === 1);
}

{
  const { window } = scenario({ desktop: true });
  const drop = syntheticDrag(window, "drop", ["Files"]);
  Object.defineProperty(drop.dataTransfer, "files", { value: { length: 1, 0: { name: "x" } } });
  window.dispatchEvent(drop);
  check("without the bridge a file drop only suppresses the default", drop.defaultPrevented);
}

// --- A failed grant paints the status bar and the rest still run ------------

{
  const { window, calls, local } = scenario({
    desktop: true,
    responder: (path) =>
      path.endsWith("blocked")
        ? {
            ok: false,
            status: 403,
            json: async () => ({
              error: { message: "path is outside every granted root", code: "outside_grants" },
            }),
          }
        : { ok: true, status: 200, json: async () => ({ granted: path }) },
  });
  window.dispatchEvent(
    new window.CustomEvent("promptforge:file-drop", {
      detail: { paths: ["C:\\blocked", "C:\\allowed"] },
    }),
  );
  await flush();
  check("a failed grant does not stop the remaining paths", calls.length === 2);
  check("a failed grant paints one local error", local.length === 1);
  check(
    "the local error carries the path, the server message, and error severity",
    local.length === 1 &&
      local[0].severity === "error" &&
      local[0].label.includes("C:\\blocked") &&
      local[0].label.includes("path is outside every granted root"),
  );
}

if (failures.length > 0) {
  console.error(`workspace-drops: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workspace-drops: all assertions passed");
