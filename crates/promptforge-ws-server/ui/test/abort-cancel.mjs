// Abort-rides-cancel contract for WorkshopSocket (step 15): aborting one
// of two in-flight chats sends `{"type":"cancel","id":N}` for that chat on
// the shared socket, settles only that chat (resolve - the tab stopped it
// deliberately), and fires onAbort so listeners clear activity state. The
// sibling chat is untouched: its deltas keep flowing on the same socket
// (no recycle, no fresh connection) and its done frame resolves it.
// Drives the socket against a scripted fake WebSocket, no DOM needed.
// Run: node test/abort-cancel.mjs
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

const bundlePath = path.join(os.tmpdir(), "promptforge-abort-cancel-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, WorkshopSocket } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
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
}
globalThis.WebSocket = FakeWebSocket;

await assertNoLeaks(lifecycle, async () => {
  const socket = new WorkshopSocket("ws://fake/ws");
  let aborts = 0;
  socket.onAbort(() => (aborts += 1));
  socket.connect();
  const wire = fakeSockets[0];
  wire.open();

  // Two chats in flight on the one socket, multiplexed by id.
  const stopper = new AbortController();
  let stoppedResolved = false;
  let stoppedError = null;
  const stoppedChat = socket
    .streamChat({ messages: [] }, { onDelta: () => {} }, stopper.signal)
    .then(
      () => {
        stoppedResolved = true;
      },
      (error) => {
        stoppedError = error;
      },
    );
  const survivorDeltas = [];
  let survivorResolved = false;
  let survivorError = null;
  const survivorChat = socket
    .streamChat(
      { messages: [] },
      { onDelta: (content) => survivorDeltas.push(content) },
      new AbortController().signal,
    )
    .then(
      () => {
        survivorResolved = true;
      },
      (error) => {
        survivorError = error;
      },
    );
  await flush();
  check(
    "both chat frames went out on the one socket",
    wire.sent.length === 2 && fakeSockets.length === 1,
  );

  wire.message({ type: "delta", id: 1, content: "doomed" });
  wire.message({ type: "delta", id: 2, content: "before" });

  // --- Abort chat 1: its cancel frame, its local settle, nothing else -------

  stopper.abort();
  await stoppedChat;
  check(
    "the abort sends the cancel frame for that chat's id",
    wire.sent.at(-1) === JSON.stringify({ type: "cancel", id: 1 }),
  );
  check(
    "the aborted chat settles locally with resolve",
    stoppedResolved === true && stoppedError === null,
  );
  check("the abort fires onAbort so listeners clear activity state", aborts === 1);
  check("the sibling chat is not settled by the abort", survivorResolved === false);
  check("the shared socket is not recycled", fakeSockets.length === 1 && wire.readyState === 1);

  // --- The sibling chat streams on and completes on the same socket ---------

  wire.message({ type: "delta", id: 2, content: " after" });
  check(
    "the sibling chat's deltas keep flowing after the abort",
    survivorDeltas.join("") === "before after",
  );
  wire.message({ type: "delta", id: 1, content: "stale" });
  wire.message({ type: "done", id: 2 });
  await survivorChat;
  check(
    "the sibling chat resolves on its own done frame",
    survivorResolved === true && survivorError === null,
  );
  stopper.abort();
  check("a second abort of the settled chat fires nothing", aborts === 1);

  // --- Abort with the socket already closed: settling locally is the job ----

  const lateStopper = new AbortController();
  let lateResolved = false;
  const lateChat = socket
    .streamChat({ messages: [] }, { onDelta: () => {} }, lateStopper.signal)
    .then(() => {
      lateResolved = true;
    });
  await flush();
  // The socket dies without its onclose ever running (the app never saw
  // the drop), so the pending chat is still held when the abort lands.
  wire.readyState = 3;
  const sentBeforeLateAbort = wire.sent.length;
  lateStopper.abort();
  await lateChat;
  check(
    "an abort on a closed socket sends no cancel frame",
    wire.sent.length === sentBeforeLateAbort,
  );
  check("an abort on a closed socket still settles the chat locally", lateResolved === true);
  check("an abort on a closed socket still fires onAbort", aborts === 2);

  socket.dispose();
});

if (failures.length > 0) {
  console.error(`abort-cancel: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("abort-cancel: all assertions passed");
process.exit(0);
