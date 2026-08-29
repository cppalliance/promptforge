// Stream-generation test for the voice client (src/ui/voice.ts, step 15):
// the server's `stream` frame announces the take's generation; interim and
// final frames tagged with an older generation are stale (a stop/restart
// race) and must be discarded; frames with no generation, or tagged frames
// arriving before any announcement, are treated as current so the client
// tolerates a server that never announces. Drives setupVoice against a
// scripted fake WebSocket and stubbed audio in a jsdom DOM.
// Run: node test/voice-stream.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { setupVoice } from "./src/ui/voice.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  loader: { ".css": "empty" },
  logLevel: "silent",
});

const bundlePath = path.join(os.tmpdir(), "promptforge-voice-stream-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, setupVoice } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// A DOM for the mic button and the composer textarea. The bundle reads the
// globals, so jsdom's window fills the gaps Node does not provide; Event
// comes from jsdom too, so dispatchEvent accepts what notifyInput builds.
const { window } = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1/",
});
globalThis.document = window.document;
globalThis.location = window.location;
globalThis.Event = window.Event;
globalThis.window = window;

// Audio stubs: the getUserMedia/AudioContext path is scripted to succeed,
// as in smoke.mjs - jsdom has no audio stack.
const fakeAudioStream = { getTracks: () => [{ stop() {} }] };
globalThis.navigator.mediaDevices = {
  getUserMedia: () => Promise.resolve(fakeAudioStream),
};
class FakeAudioContext {
  constructor() {
    this.destination = {};
    this.audioWorklet = { addModule: () => Promise.resolve() };
  }
  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }
  close() {
    return Promise.resolve();
  }
}
class FakeAudioWorkletNode {
  constructor() {
    this.port = { onmessage: null };
  }
  connect() {}
  disconnect() {}
}
window.AudioContext = FakeAudioContext;
globalThis.AudioContext = FakeAudioContext;
globalThis.AudioWorkletNode = FakeAudioWorkletNode;

// A scripted /voice socket. Opens asynchronously like a real one; the
// test drives server frames through message().
const sockets = [];
class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  constructor(url) {
    this.url = url;
    this.readyState = FakeWebSocket.CONNECTING;
    this.closed = false;
    this.sent = [];
    this.listeners = new Map();
    sockets.push(this);
    setTimeout(() => {
      this.readyState = FakeWebSocket.OPEN;
      this.dispatch("open", {});
    }, 0);
  }
  addEventListener(type, listener, options) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push({ listener, once: options?.once === true });
  }
  dispatch(type, event) {
    const entries = this.listeners.get(type) ?? [];
    this.listeners.set(
      type,
      entries.filter((entry) => !entry.once),
    );
    for (const entry of entries) entry.listener(event);
  }
  send(data) {
    this.sent.push(data);
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.readyState = FakeWebSocket.CLOSED;
    this.dispatch("close", {});
  }
  // Test-side control, not part of the WebSocket surface.
  message(frame) {
    this.dispatch("message", { data: JSON.stringify(frame) });
  }
}
window.WebSocket = FakeWebSocket;
globalThis.WebSocket = FakeWebSocket;

// startVoice crosses several await points (getUserMedia, socket open,
// worklet load) before sending "start"; poll until the take is live.
async function waitFor(condition) {
  for (let attempt = 0; attempt < 50; attempt++) {
    if (condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  return false;
}

const statusBar = { showLocal() {}, setRecording() {} };

await assertNoLeaks(lifecycle, async () => {
  const mic = window.document.createElement("button");
  const input = window.document.createElement("textarea");
  window.document.body.append(mic, input);
  const handle = setupVoice({ mic, input }, statusBar);

  // --- The stream frame sets the generation; matching frames apply -------

  mic.click();
  check(
    "the mic click opens a /voice socket and sends start",
    await waitFor(() => sockets.length === 1 && sockets[0].sent.includes("start")),
  );
  const socket = sockets[0];
  socket.message({ type: "stream", generation: 2 });
  socket.message({ type: "interim", committed: "ask not", tentative: "", generation: 2 });
  check("a current-generation interim splices into the textarea", input.value === "ask not");

  // --- Stale frames are discarded -----------------------------------------

  socket.message({ type: "interim", committed: "STALE", tentative: "", generation: 1 });
  check("a stale interim is discarded", input.value === "ask not");
  socket.message({ type: "final", text: "stale final", frames: 1, generation: 1 });
  check("a stale final does not finish the take", input.value === "ask not" && input.readOnly);
  check("a stale final does not close the socket", !socket.closed);

  // --- A missing generation is treated as current --------------------------

  socket.message({ type: "interim", committed: "ask not what", tentative: "" });
  check("an interim with no generation is treated as current", input.value === "ask not what");
  socket.message({ type: "final", text: "ask not what you can do", frames: 64, generation: 2 });
  check(
    "the current generation's final finishes the take",
    input.value === "ask not what you can do" && !input.readOnly,
  );
  check("the final closes the socket", socket.closed);

  // --- Tagged frames before any announcement are treated as current --------

  input.value = "";
  input.setSelectionRange(0, 0);
  mic.click();
  check(
    "a second mic click opens a fresh /voice socket",
    await waitFor(() => sockets.length === 2 && sockets[1].sent.includes("start")),
  );
  sockets[1].message({ type: "interim", committed: "later take", tentative: "", generation: 5 });
  check(
    "a tagged interim before any stream frame is treated as current",
    input.value === "later take",
  );

  handle.dispose();
});

if (failures.length > 0) {
  console.error(`voice-stream: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("voice-stream: all assertions passed");
process.exit(0);
