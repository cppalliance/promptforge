// Workbench frame and event sends for WorkshopSocket (step 15): a pushed
// workbench frame routes to the onWorkbench emitter; workbench pushes that
// arrive before ready() replay through the boot queue in arrival order
// beside status and models pushes; selectModel/switchProfile put the
// documented event frames on the wire and return true; when the socket is
// down they send nothing and return false, so the caller can surface the
// failure. Drives the socket against a scripted fake WebSocket, no DOM
// needed.
// Run: node test/workbench-frames.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { WorkshopSocket } from "./src/services/workshop-socket.ts";
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
});

const bundlePath = path.join(os.tmpdir(), "promptforge-workbench-frames-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, WorkshopSocket } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const fakeSockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  sent = [];
  onopen = null;
  onclose = null;
  onerror = null;
  onmessage = null;
  constructor(url) {
    this.url = url;
    fakeSockets.push(this);
  }
  send(data) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
  }
  // Test-side controls, not part of the WebSocket surface.
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  message(frame) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
  drop() {
    this.readyState = 3;
    this.onclose?.();
  }
}
globalThis.WebSocket = FakeWebSocket;

function workbenchFrame(overrides = {}) {
  return {
    type: "workbench",
    profiles: ["main", "coding"],
    active: "main",
    switching: null,
    selected: "test-model",
    chat_ready: true,
    ...overrides,
  };
}

function statusFrame(label) {
  return {
    type: "status",
    label,
    description: "",
    severity: "info",
    activity: null,
    progress: null,
  };
}

await assertNoLeaks(lifecycle, async () => {
  // --- A workbench push routes to the onWorkbench emitter -------------------

  const socket = new WorkshopSocket("ws://fake/ws");
  const snapshots = [];
  socket.onWorkbench((frame) => snapshots.push(frame));
  socket.ready();
  socket.connect();
  fakeSockets[0].open();
  fakeSockets[0].message(workbenchFrame());
  check("a workbench push reaches the onWorkbench emitter", snapshots.length === 1);
  check(
    "the emitted frame carries the wire fields verbatim",
    snapshots[0]?.active === "main" &&
      snapshots[0]?.selected === "test-model" &&
      snapshots[0]?.switching === null &&
      snapshots[0]?.chat_ready === true &&
      snapshots[0]?.profiles.join(",") === "main,coding",
  );

  // --- Send methods put the documented event frames on the wire -------------

  check("selectModel on an open socket reports success", socket.selectModel("gpt-test") === true);
  check(
    "selectModel sends one select_model frame naming the model",
    fakeSockets[0].sent.at(-1) === JSON.stringify({ type: "select_model", model: "gpt-test" }),
  );
  check("switchProfile on an open socket reports success", socket.switchProfile("coding") === true);
  check(
    "switchProfile sends one switch_profile frame naming the profile",
    fakeSockets[0].sent.at(-1) === JSON.stringify({ type: "switch_profile", name: "coding" }),
  );

  // --- Socket down: the send methods fail loudly instead of dropping --------

  const sentBeforeDrop = fakeSockets[0].sent.length;
  fakeSockets[0].drop();
  check("selectModel on a downed socket reports failure", socket.selectModel("gpt-test") === false);
  check("switchProfile on a downed socket reports failure", socket.switchProfile("coding") === false);
  check(
    "a downed socket carries no event frames",
    fakeSockets[0].sent.length === sentBeforeDrop,
  );
  socket.dispose();

  // --- Workbench pushes replay through the boot queue ------------------------

  const bootSocket = new WorkshopSocket("ws://fake/ws");
  const replayed = [];
  bootSocket.onStatus((frame) => replayed.push(`status:${frame.label}`));
  bootSocket.onWorkbench((frame) => replayed.push(`workbench:${frame.active}`));
  bootSocket.connect();
  const bootWire = fakeSockets.at(-1);
  bootWire.open();
  bootWire.message(statusFrame("s1"));
  bootWire.message(workbenchFrame({ active: "main" }));
  bootWire.message(workbenchFrame({ active: "coding" }));
  check("no workbench push is delivered before ready()", replayed.length === 0);
  bootSocket.ready();
  check(
    "ready() replays workbench pushes in arrival order beside status pushes",
    replayed.join(",") === "status:s1,workbench:main,workbench:coding",
  );
  bootWire.message(workbenchFrame({ active: "late" }));
  check(
    "a workbench push after ready() delivers immediately",
    replayed.length === 4 && replayed.at(-1) === "workbench:late",
  );
  bootSocket.dispose();

  // --- A socket that never opened also fails the send methods ---------------

  const closedSocket = new WorkshopSocket("ws://fake/ws");
  check(
    "selectModel before any connection reports failure",
    closedSocket.selectModel("gpt-test") === false,
  );
  check(
    "switchProfile before any connection reports failure",
    closedSocket.switchProfile("coding") === false,
  );
  closedSocket.dispose();
});

if (failures.length > 0) {
  console.error(`workbench-frames: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workbench-frames: all assertions passed");
process.exit(0);
