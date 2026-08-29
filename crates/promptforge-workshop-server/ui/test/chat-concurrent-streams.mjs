// Two chat streams on the one workshop socket (src/services/
// workshop-provider.ts over workshop-socket.ts): the socket multiplexes
// concurrent generations by request id, so two Agent tabs can stream at
// the same time. Drives two provider streams over a scripted fake socket
// with interleaved id-tagged delta frames and checks each stream renders
// only its own content, one stream's done frame settles its promise while
// the other is still mid-flight (settlements are independent, not held
// until every stream ends), and both settle on their own terminal frames.
// Run: node test/chat-concurrent-streams.mjs
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
      export { WorkshopProvider } from "./src/services/workshop-provider.ts";
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

const bundlePath = path.join(os.tmpdir(), "promptforge-chat-concurrent-streams-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, WorkshopSocket, WorkshopProvider } = await import(
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

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// One tab's generation request, in the ChatRequest shape the engine hands
// the provider.
function tabRequest(text) {
  return {
    messages: [{ id: "user-turn", role: "user", blocks: [{ type: "text", text }] }],
    options: { model: "test-model" },
    signal: new AbortController().signal,
  };
}

await assertNoLeaks(lifecycle, async () => {
  const socket = new WorkshopSocket("ws://fake/ws");
  socket.ready();
  socket.connect();
  const wire = fakeSockets[0];
  wire.open();

  // Both tabs share one provider, exactly as the AgentController wires it.
  const provider = new WorkshopProvider(socket);
  const eventsA = [];
  const eventsB = [];
  const streamA = provider.streamChat(tabRequest("first tab?"), (event) => eventsA.push(event));
  const streamB = provider.streamChat(tabRequest("second tab?"), (event) => eventsB.push(event));

  // streamChat awaits the (already open) socket before sending, so the
  // frames land a few microtasks after the calls.
  const deadline = Date.now() + 2000;
  const chatFrames = () => wire.sent.map((raw) => JSON.parse(raw)).filter((f) => f.type === "chat");
  while (chatFrames().length < 2 && Date.now() < deadline) {
    await sleep(5);
  }
  const frames = chatFrames();
  check("both tabs put their chat frames on the one socket", frames.length === 2);
  const [idA, idB] = frames.map((frame) => frame.id);
  check(
    "the two streams carry distinct numeric ids",
    typeof idA === "number" && typeof idB === "number" && idA !== idB,
  );

  // The server interleaves the two replies on the shared socket; the
  // second stream finishes while the first is still streaming.
  wire.message({ type: "delta", content: "alpha-1 ", id: idA });
  wire.message({ type: "delta", content: "beta-1 ", id: idB });
  wire.message({ type: "delta", content: "alpha-2 ", id: idA });
  wire.message({ type: "delta", content: "beta-2", id: idB });
  wire.message({ type: "done", id: idB });

  // Independent settlement: B's promise must resolve on its own done
  // frame while A is still mid-flight (its final delta not yet sent). A
  // bug holding every settlement until all streams end would leave B
  // pending here and lose the race to the timeout.
  const bSettledMidFlight = await Promise.race([
    streamB.then(() => true),
    sleep(500).then(() => false),
  ]);
  check("one stream's done frame settles its promise while the other still streams", bSettledMidFlight);

  wire.message({ type: "delta", content: "alpha-3", id: idA });
  wire.message({ type: "done", id: idA });

  await Promise.all([streamA, streamB]);

  const text = (events) =>
    events.filter((event) => event.type === "text_delta").map((event) => event.delta).join("");
  check("the first tab's stream carries only its own deltas", text(eventsA) === "alpha-1 alpha-2 alpha-3");
  check("the second tab's stream carries only its own deltas", text(eventsB) === "beta-1 beta-2");
  check(
    "both streams finish on their own terminal frames",
    eventsA.at(-1)?.type === "finish" && eventsB.at(-1)?.type === "finish",
  );
  const startA = eventsA.find((event) => event.type === "message_start");
  const startB = eventsB.find((event) => event.type === "message_start");
  check(
    "each stream announces its own assistant message",
    startA !== undefined && startB !== undefined && startA.message.id !== startB.message.id,
  );
  const blockIds = (events) =>
    new Set(events.filter((event) => event.type === "text_delta").map((event) => event.blockId));
  check(
    "the streams never share a text block",
    blockIds(eventsA).size === 1 &&
      blockIds(eventsB).size === 1 &&
      !blockIds(eventsA).has([...blockIds(eventsB)][0]),
  );
  socket.dispose();
});

if (failures.length > 0) {
  console.error(`chat-concurrent-streams: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("chat-concurrent-streams: all assertions passed");
process.exit(0);
