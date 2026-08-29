// Close-after-reasoning contract for WorkshopSocket (step 5): only answer
// delta frames mark a reply started. A socket that closes after nothing but
// reasoning frames rejects the chat with the close error - a reasoning model
// that dies before its first answer token is a failed turn, not a completed
// one with an empty answer - while a close after an answer delta still
// resolves. Drives the socket against a scripted fake WebSocket, no DOM
// needed.
// Run: node test/reasoning-close-rejects.mjs
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

const bundlePath = path.join(os.tmpdir(), "promptforge-reasoning-close-test.mjs");
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
  serverClose() {
    this.readyState = 3;
    this.onclose?.();
  }
}
globalThis.WebSocket = FakeWebSocket;

await assertNoLeaks(lifecycle, async () => {
  // --- Reasoning frames then close: the chat rejects ------------------------

  const reasoningSocket = new WorkshopSocket("ws://fake/ws");
  reasoningSocket.connect();
  fakeSockets[0].open();
  const reasoning = [];
  let reasoningResolved = false;
  let reasoningError = null;
  const reasoningChat = reasoningSocket
    .streamChat(
      { messages: [] },
      { onDelta: () => {}, onReasoning: (content) => reasoning.push(content) },
      new AbortController().signal,
    )
    .then(
      () => {
        reasoningResolved = true;
      },
      (error) => {
        reasoningError = error;
      },
    );
  await flush();
  fakeSockets[0].message({ type: "reasoning", id: 1, content: "consider the ask" });
  fakeSockets[0].message({ type: "reasoning", id: 1, content: " then answer" });
  fakeSockets[0].serverClose();
  await reasoningChat;
  check(
    "a close after nothing but reasoning frames rejects the chat",
    reasoningError instanceof Error && reasoningResolved === false,
  );
  check(
    "the rejection carries the existing close error",
    reasoningError?.message === "the workshop socket closed before the reply completed",
  );
  check(
    "the reasoning frames still reached their handler before the close",
    reasoning.join("") === "consider the ask then answer",
  );
  reasoningSocket.dispose();

  // --- An answer delta then close: the chat still resolves ------------------

  const answerSocket = new WorkshopSocket("ws://fake/ws");
  answerSocket.connect();
  fakeSockets[1].open();
  let answerResolved = false;
  let answerError = null;
  const answerChat = answerSocket
    .streamChat({ messages: [] }, { onDelta: () => {} }, new AbortController().signal)
    .then(
      () => {
        answerResolved = true;
      },
      (error) => {
        answerError = error;
      },
    );
  await flush();
  fakeSockets[1].message({ type: "delta", id: 1, content: "partial" });
  fakeSockets[1].serverClose();
  await answerChat;
  check(
    "a close after an answer delta still resolves the chat",
    answerResolved === true && answerError === null,
  );
  answerSocket.dispose();
});

if (failures.length > 0) {
  console.error(`reasoning-close-rejects: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("reasoning-close-rejects: all assertions passed");
process.exit(0);
