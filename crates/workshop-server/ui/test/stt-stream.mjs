// Browser Realtime transcription service, pinned to the canonical wire
// fixtures shared with the Rust implementation. Run: node test/stt-stream.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const fixtures = path.join(uiDir, "..", "..", "..", "gateway-stt", "tests", "fixtures", "realtime");
const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { RealtimeTranscriptionService } from "./src/services/realtime-transcription.ts";
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

const { lifecycle, RealtimeTranscriptionService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);
const client = JSON.parse(await readFile(path.join(fixtures, "client-events.json"), "utf8"));
const server = JSON.parse(await readFile(path.join(fixtures, "server-events.json"), "utf8"));
globalThis.location = new URL("http://127.0.0.1:7910/");

class ScriptedSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  constructor(url) {
    this.url = url;
    this.readyState = ScriptedSocket.CONNECTING;
    this.sent = [];
    this.listeners = new Map();
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
    this.sent.push(JSON.parse(data));
  }
  close() {
    if (this.readyState === ScriptedSocket.CLOSED) return;
    this.readyState = ScriptedSocket.CLOSED;
    this.dispatch("close", {});
  }
  open() {
    this.readyState = ScriptedSocket.OPEN;
    this.dispatch("open", {});
  }
  message(frame) {
    this.dispatch("message", { data: JSON.stringify(frame) });
  }
}

await assertNoLeaks(lifecycle, async () => {
  const sockets = [];
  const service = new RealtimeTranscriptionService({
    prompt: "meeting notes",
    eventId: (() => {
      const ids = [
        "client_update_1",
        "client_append_1",
        "client_commit_1",
        "client_clear_1",
      ];
      return () => ids.shift();
    })(),
    socket: (url) => {
      const socket = new ScriptedSocket(url);
      sockets.push(socket);
      return socket;
    },
  });
  const states = [];
  const snapshots = [];
  const completions = [];
  const failures = [];
  const errors = [];
  service.onState((value) => states.push(value));
  service.onSnapshot((value) => snapshots.push(value));
  service.onCompleted((value) => completions.push(value));
  service.onFailed((value) => failures.push(value));
  service.onError((value) => errors.push(value));

  assert.equal(sockets.length, 1);
  assert.match(sockets[0].url, /\/v1\/realtime$/);
  sockets[0].open();
  sockets[0].message(server.session_created);
  assert.deepEqual(sockets[0].sent, [client.session_update]);
  sockets[0].message(server.session_updated);
  assert.equal(service.state, "ready");
  assert.deepEqual(states, ["ready"]);

  service.append(Uint8Array.from([0, 0, 1, 0, 255, 255]).buffer);
  service.commit();
  service.clear();
  assert.deepEqual(sockets[0].sent.slice(1), [
    client.input_audio_buffer_append,
    { ...client.input_audio_buffer_commit, event_id: "client_commit_1" },
    { ...client.input_audio_buffer_clear, event_id: "client_clear_1" },
  ]);

  sockets[0].message(server.input_audio_buffer_committed);
  sockets[0].message(server.transcription_hypothesis);
  sockets[0].message(server.transcription_delta);
  sockets[0].message(server.transcription_completed);
  sockets[0].message(server.transcription_failed);
  sockets[0].message(server.error_correlated);
  assert.deepEqual(snapshots, [{ itemId: "item_alpha", text: "Hello, world" }]);
  assert.deepEqual(completions, [{ itemId: "item_alpha", transcript: "Hello, world" }]);
  assert.deepEqual(failures, [{ itemId: "item_beta", code: "transcription_failed" }]);
  assert.deepEqual(errors, [
    {
      code: "unsupported_model",
      scope: "event",
      eventId: "client_bad_update",
      recoverable: true,
    },
  ]);

  service.dispose();
  assert.equal(sockets[0].readyState, ScriptedSocket.CLOSED);
});
