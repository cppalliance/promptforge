// Browser Realtime transcription service, pinned to the canonical wire
// fixtures shared with the Rust implementation. Run: node test/stt-stream.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { mock } from "node:test";
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

await assertNoLeaks(lifecycle, async () => {
  const sockets = [];
  const service = new RealtimeTranscriptionService({
    eventId: () => "client_decoder_update",
    socket: (url) => {
      const socket = new ScriptedSocket(url);
      sockets.push(socket);
      return socket;
    },
  });
  const committed = [];
  const completions = [];
  const errors = [];
  service.onCommitted((value) => committed.push(value));
  service.onCompleted((value) => completions.push(value));
  service.onError((value) => errors.push(value));

  sockets[0].open();
  sockets[0].message(server.session_created);
  sockets[0].message(server.session_updated);
  sockets[0].message({
    ...server.input_audio_buffer_committed,
    unexpected: true,
  });
  sockets[0].message({
    ...server.transcription_completed,
    content_index: 1,
  });
  sockets[0].message({
    event_id: "evt_future",
    type: "response.created",
  });

  assert.deepEqual(committed, []);
  assert.deepEqual(completions, []);
  assert.deepEqual(
    errors,
    Array.from({ length: 3 }, () => ({
      code: "invalid_server_event",
      scope: "session",
      eventId: null,
      recoverable: true,
    })),
  );
  service.dispose();
});

await assertNoLeaks(lifecycle, async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const sockets = [];
    const service = new RealtimeTranscriptionService({
      eventId: () => "client_fallback_update",
      socket: (url) => {
        const socket = new ScriptedSocket(url);
        sockets.push(socket);
        return socket;
      },
    });
    const snapshots = [];
    const completions = [];
    const errors = [];
    service.onSnapshot((value) => snapshots.push(value));
    service.onCompleted((value) => completions.push(value));
    service.onError((value) => errors.push(value));

    const fallbackSessionUpdated = {
      ...server.session_updated,
      session: {
        ...server.session_updated.session,
        include: [],
      },
    };
    sockets[0].open();
    sockets[0].message(server.session_created);
    sockets[0].message(fallbackSessionUpdated);
    sockets[0].message({
      ...server.transcription_delta,
      event_id: "evt_stale_delta_1",
      item_id: "item_shared",
      delta: "stale",
    });
    sockets[0].message({
      ...server.transcription_delta,
      event_id: "evt_stale_delta_2",
      item_id: "item_shared",
      delta: " prefix",
    });

    sockets[0].close();
    mock.timers.tick(1_000);
    assert.equal(sockets.length, 2, "the fallback session reconnects deterministically");
    sockets[1].open();
    sockets[1].message(server.session_created);
    sockets[1].message(fallbackSessionUpdated);
    sockets[1].message({
      ...server.transcription_delta,
      event_id: "evt_fresh_delta_1",
      item_id: "item_shared",
      delta: "fresh",
    });
    sockets[1].message({
      ...server.transcription_delta,
      event_id: "evt_isolated_delta_1",
      item_id: "item_isolated",
      delta: "other",
    });
    sockets[1].message({
      ...server.transcription_delta,
      event_id: "evt_fresh_delta_2",
      item_id: "item_shared",
      delta: " transcript",
    });
    sockets[1].message({
      ...server.transcription_delta,
      event_id: "evt_isolated_delta_2",
      item_id: "item_isolated",
      delta: " item",
    });
    sockets[1].message({
      ...server.transcription_delta,
      event_id: "evt_invalid_fallback_delta",
      item_id: "item_invalid",
      unexpected: true,
    });
    sockets[1].message({
      ...server.transcription_completed,
      event_id: "evt_fresh_completed",
      item_id: "item_shared",
      transcript: "fresh transcript",
    });

    sockets[1].message(server.session_updated);
    sockets[1].message({
      ...server.transcription_delta,
      event_id: "evt_ignored_after_negotiation",
      item_id: "item_isolated",
      delta: " ignored",
    });
    sockets[1].message({
      ...server.transcription_hypothesis,
      event_id: "evt_later_hypothesis",
      item_id: "item_isolated",
      transcript: "hypothesis wins",
      finalized: "",
      agreed: "",
      tentative: "hypothesis wins",
    });
    sockets[1].message({
      ...server.transcription_completed,
      event_id: "evt_isolated_completed",
      item_id: "item_isolated",
      transcript: "hypothesis wins",
    });

    assert.deepEqual(snapshots, [
      { itemId: "item_shared", text: "stale" },
      { itemId: "item_shared", text: "stale prefix" },
      { itemId: "item_shared", text: "fresh" },
      { itemId: "item_isolated", text: "other" },
      { itemId: "item_shared", text: "fresh transcript" },
      { itemId: "item_isolated", text: "other item" },
      { itemId: "item_isolated", text: "hypothesis wins" },
    ]);
    assert.deepEqual(completions, [
      { itemId: "item_shared", transcript: "fresh transcript" },
      { itemId: "item_isolated", transcript: "hypothesis wins" },
    ]);
    assert.deepEqual(errors.at(-1), {
      code: "invalid_server_event",
      scope: "session",
      eventId: null,
      recoverable: true,
    });

    service.dispose();
  } finally {
    mock.timers.reset();
  }
});

await assertNoLeaks(lifecycle, async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    let attempts = 0;
    const service = new RealtimeTranscriptionService({
      socket: () => {
        attempts += 1;
        throw new Error("gateway is still starting");
      },
    });

    assert.equal(service.state, "unavailable");
    assert.equal(attempts, 1);
    const schedule = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000];
    for (const [index, delay] of schedule.entries()) {
      mock.timers.tick(delay - 1);
      assert.equal(
        attempts,
        index + 1,
        `retry ${index + 1} does not run before its ${delay} ms delay`,
      );
      mock.timers.tick(1);
      assert.equal(
        attempts,
        index + 2,
        `retry ${index + 1} runs at its ${delay} ms delay`,
      );
    }

    service.dispose();
    mock.timers.tick(30_000);
    assert.equal(attempts, 8, "disposal cancels the pending capped retry");
  } finally {
    mock.timers.reset();
  }
});

await assertNoLeaks(lifecycle, async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const sockets = [];
    let attempts = 0;
    const service = new RealtimeTranscriptionService({
      socket: (url) => {
        attempts += 1;
        if (attempts === 1) {
          throw new Error("gateway is still starting");
        }
        const socket = new ScriptedSocket(url);
        sockets.push(socket);
        return socket;
      },
    });

    mock.timers.tick(1000);
    assert.equal(attempts, 2, "an initial failure reconnects without another mic click");
    sockets[0].open();
    sockets[0].message(server.session_created);
    sockets[0].message(server.session_updated);
    assert.equal(service.state, "ready");

    sockets[0].close();
    mock.timers.tick(999);
    assert.equal(attempts, 2, "readiness resets the reconnect delay to one second");
    mock.timers.tick(1);
    assert.equal(attempts, 3, "an established connection reconnects on the reset delay");
    const racing = sockets[1];
    service.dispose();
    racing.dispatch("close", {});
    mock.timers.tick(30_000);
    assert.equal(attempts, 3, "disposal cancels a reconnect even as its socket is created");
  } finally {
    mock.timers.reset();
  }
});
