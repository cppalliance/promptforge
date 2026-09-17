// Boot-queue test for WorkshopSocket (step 13): status/models pushes that
// arrive before ready() are queued, then replayed in arrival order when
// ready() is called; pushes after ready() deliver immediately; ready() is
// idempotent; disposal and a connection drop each clear the queue; the
// queue is bounded at BOOT_QUEUE_CAP with drop-oldest overflow. Drives the
// socket against a scripted fake WebSocket, no DOM needed.
// Run: node test/boot-queue.mjs
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
      export { WorkshopSocket, BOOT_QUEUE_CAP } from "./src/services/workshop-socket.ts";
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

const bundlePath = path.join(os.tmpdir(), "promptforge-boot-queue-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, WorkshopSocket, BOOT_QUEUE_CAP } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const fakeSockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  closed = false;
  onopen = null;
  onclose = null;
  onerror = null;
  onmessage = null;
  constructor(url) {
    this.url = url;
    fakeSockets.push(this);
  }
  send() {}
  close() {
    this.closed = true;
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
  // --- Queue before ready, replay in arrival order, immediate after -------

  const socket = new WorkshopSocket("ws://fake/ws");
  const delivered = [];
  socket.onStatus((frame) => delivered.push(`status:${frame.label}`));
  socket.onModels((models) => delivered.push(`models:${models[0]?.id}`));
  socket.connect();
  fakeSockets[0].open();

  fakeSockets[0].message(statusFrame("s1"));
  fakeSockets[0].message({ type: "models", models: [{ id: "m1", description: "" }] });
  fakeSockets[0].message(statusFrame("s2"));
  check("no push is delivered before ready()", delivered.length === 0);

  socket.ready();
  check(
    "ready() replays queued pushes in arrival order, status and models interleaved",
    delivered.join(",") === "status:s1,models:m1,status:s2",
  );

  fakeSockets[0].message(statusFrame("s3"));
  check(
    "a push after ready() delivers immediately",
    delivered.length === 4 && delivered.at(-1) === "status:s3",
  );

  socket.ready();
  check("a second ready() replays nothing", delivered.length === 4);
  socket.dispose();

  // --- Disposal clears the queue -------------------------------------------

  const disposedSocket = new WorkshopSocket("ws://fake/ws");
  const afterDisposal = [];
  disposedSocket.onStatus((frame) => afterDisposal.push(frame.label));
  disposedSocket.connect();
  fakeSockets[1].open();
  fakeSockets[1].message(statusFrame("queued"));
  disposedSocket.dispose();
  disposedSocket.ready();
  check(
    "disposal clears the queue: ready() after dispose delivers nothing",
    afterDisposal.length === 0,
  );

  // --- The cap drops the oldest pushes --------------------------------------

  const cappedSocket = new WorkshopSocket("ws://fake/ws");
  const capped = [];
  cappedSocket.onStatus((frame) => capped.push(frame.label));
  cappedSocket.connect();
  fakeSockets[2].open();
  const overflow = 3;
  for (let i = 1; i <= BOOT_QUEUE_CAP + overflow; i++) {
    fakeSockets[2].message(statusFrame(`s${i}`));
  }
  cappedSocket.ready();
  check("the queue never holds more than BOOT_QUEUE_CAP pushes", capped.length === BOOT_QUEUE_CAP);
  check(
    "overflow drops the oldest pushes, keeping the newest",
    capped[0] === `s${overflow + 1}` && capped.at(-1) === `s${BOOT_QUEUE_CAP + overflow}`,
  );
  cappedSocket.dispose();

  // --- A dropped connection clears its queued pushes ------------------------

  const droppedSocket = new WorkshopSocket("ws://fake/ws");
  const afterDrop = [];
  droppedSocket.onStatus((frame) => afterDrop.push(frame.label));
  droppedSocket.connect();
  fakeSockets[3].open();
  fakeSockets[3].message(statusFrame("stale"));
  fakeSockets[3].drop();
  droppedSocket.ready();
  check(
    "a connection drop before ready() clears its queued pushes",
    afterDrop.length === 0,
  );
  droppedSocket.dispose();
});

if (failures.length > 0) {
  console.error(`boot-queue: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("boot-queue: all assertions passed");
process.exit(0);
