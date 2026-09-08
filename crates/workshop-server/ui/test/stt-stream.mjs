// Browser Realtime transcription service, pinned to the canonical wire
// fixtures shared with the Rust implementation. Run: node test/stt-stream.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { mock } from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const fixtures = path.join(uiDir, "..", "..", "..", "gateway-stt", "tests", "fixtures", "realtime");
const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { RealtimeTranscriptionService } from "./src/services/realtime-transcription.ts";
      export { SpeechCaptureService } from "./src/services/speech-capture.ts";
      export { setupStt, textareaSttTarget } from "./src/ui/stt.ts";
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

const {
  lifecycle,
  RealtimeTranscriptionService,
  SpeechCaptureService,
  setupStt,
  textareaSttTarget,
} = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);
const client = JSON.parse(await readFile(path.join(fixtures, "client-events.json"), "utf8"));
const server = JSON.parse(await readFile(path.join(fixtures, "server-events.json"), "utf8"));
globalThis.location = new URL("http://127.0.0.1:7910/");

{
  const dom = new JSDOM("<!doctype html><textarea></textarea>");
  const textarea = dom.window.document.querySelector("textarea");
  const target = textareaSttTarget(textarea);

  textarea.value = "First test alpha";
  textarea.setSelectionRange(textarea.value.length, textarea.value.length);
  const append = target.insertionContext();
  assert.deepEqual(append, {
    range: { start: 16, end: 16 },
    original: "",
    compositionPrefix: " ",
  });

  textarea.value += " ";
  assert.equal(
    append.compositionPrefix,
    " ",
    "a captured textarea composition prefix is immutable",
  );
  textarea.setSelectionRange(textarea.value.length, textarea.value.length);
  assert.equal(
    target.insertionContext().compositionPrefix,
    "",
    "existing textarea whitespace prevents a composition separator",
  );

  textarea.value = "First test alpha";
  textarea.setSelectionRange(6, 10);
  assert.deepEqual(
    target.insertionContext(),
    {
      range: { start: 6, end: 10 },
      original: "test",
      compositionPrefix: "",
    },
    "a textarea selection is captured without a composition separator",
  );

  textarea.value = "alphaBeta";
  textarea.setSelectionRange(5, 5);
  assert.deepEqual(
    target.insertionContext(),
    {
      range: { start: 5, end: 5 },
      original: "",
      compositionPrefix: "",
    },
    "a mid-word textarea insertion receives no composition separator",
  );
}

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
  const dom = new JSDOM("<!doctype html><button></button><textarea></textarea>");
  const mic = dom.window.document.querySelector("button");
  const textarea = dom.window.document.querySelector("textarea");
  const previousEvent = globalThis.Event;
  globalThis.Event = dom.window.Event;
  try {
    const socket = new ScriptedSocket("/v1/realtime");
    const realtime = new RealtimeTranscriptionService({ socket: () => socket });
    socket.open();
    socket.message(server.session_created);
    socket.message(server.session_updated);

    let finishCaptureStart = null;
    const capture = new SpeechCaptureService({
      open: () =>
        new Promise((resolve) => {
          finishCaptureStart = () =>
            resolve({
              clear() {},
              async stop() {},
              dispose() {},
            });
        }),
    });
    const status = {
      recording: false,
      showLocal() {},
      setRecording(recording) {
        this.recording = recording;
      },
    };
    const stt = setupStt(
      { mic, input: textareaSttTarget(textarea) },
      status,
      () => null,
      capture,
      realtime,
    );

    textarea.value = "old target keep";
    textarea.setSelectionRange(4, 10);
    textarea.focus();
    mic.click();
    assert.equal(typeof finishCaptureStart, "function");
    assert.equal(textarea.readOnly, false, "the textarea remains editable during startup");

    textarea.value = "edited live tail";
    textarea.setSelectionRange(7, 11);
    finishCaptureStart();
    for (let turn = 0; turn < 4 && !status.recording; turn++) {
      await Promise.resolve();
    }
    assert.equal(textarea.readOnly, true, "successful startup locks the current textarea context");

    socket.message({
      ...server.transcription_hypothesis,
      event_id: "evt_delayed_textarea_hypothesis",
      item_id: "item_delayed_textarea",
      transcript: "spoken",
      finalized: "",
      agreed: "",
      tentative: "spoken",
    });
    assert.equal(textarea.value, "edited spoken tail");
    stt.discardIfRecording();
    assert.equal(
      textarea.value,
      "edited live tail",
      "textarea rollback restores the selection captured after delayed startup",
    );
    assert.equal(textarea.selectionStart, 11);
    assert.equal(dom.window.document.activeElement, textarea);

    stt.dispose();
    capture.dispose();
    realtime.dispose();
  } finally {
    globalThis.Event = previousEvent;
    dom.window.close();
  }
});

await assertNoLeaks(lifecycle, async () => {
  const dom = new JSDOM("<!doctype html><button></button><textarea></textarea>");
  const mic = dom.window.document.querySelector("button");
  const textarea = dom.window.document.querySelector("textarea");
  const previousEvent = globalThis.Event;
  globalThis.Event = dom.window.Event;
  try {
    let nextEventId = 1;
    const socket = new ScriptedSocket("/v1/realtime");
    const realtime = new RealtimeTranscriptionService({
      eventId: () => `client_once_${nextEventId++}`,
      socket: () => socket,
    });
    socket.open();
    socket.message(server.session_created);
    socket.message(server.session_updated);

    const captureTrace = [];
    const capture = new SpeechCaptureService({
      async open() {
        return {
          clear() {
            captureTrace.push("clear");
          },
          async stop() {
            captureTrace.push("stop");
          },
          dispose() {},
        };
      },
    });
    const status = {
      local: [],
      recording: [],
      showLocal(label, severity) {
        this.local.push({ label, severity });
      },
      setRecording(recording) {
        this.recording.push(recording);
      },
    };
    const stt = setupStt(
      { mic, input: textareaSttTarget(textarea) },
      status,
      () => null,
      capture,
      realtime,
    );

    mic.click();
    for (let turn = 0; turn < 4 && status.recording.at(-1) !== true; turn++) {
      await Promise.resolve();
    }
    captureTrace.length = 0;
    status.local.length = 0;
    status.recording.length = 0;

    socket.message(server.error_uncorrelated);

    assert.deepEqual(captureTrace, ["clear", "stop"]);
    assert.deepEqual(status.recording, [false]);
    assert.deepEqual(status.local, [
      {
        label: "Dictation is temporarily unavailable. Try again.",
        severity: "error",
      },
    ]);
    assert.equal(
      socket.sent.filter(
        (event) => event.type === "input_audio_buffer.clear",
      ).length,
      1,
      "one decoded failure produces one reducer-owned wire clear",
    );

    stt.dispose();
    capture.dispose();
    realtime.dispose();
  } finally {
    globalThis.Event = previousEvent;
    dom.window.close();
  }
});

await assertNoLeaks(lifecycle, async () => {
  const dom = new JSDOM("<!doctype html><button></button><textarea></textarea>");
  const mic = dom.window.document.querySelector("button");
  const textarea = dom.window.document.querySelector("textarea");
  const previousEvent = globalThis.Event;
  globalThis.Event = dom.window.Event;
  try {
    const trace = [];
    const socket = new ScriptedSocket("/v1/realtime");
    const realtime = new RealtimeTranscriptionService({
      eventId: () => "client_loss_update",
      socket: () => socket,
    });
    socket.open();
    socket.message(server.session_created);
    socket.message(server.session_updated);

    const capture = new SpeechCaptureService({
      async open() {
        return {
          clear() {
            trace.push("capture.clear");
          },
          async stop() {
            trace.push("capture.stop");
          },
          dispose() {},
        };
      },
    });
    const status = {
      showLocal(label) {
        trace.push(`status.local:${label}`);
      },
      setRecording(recording) {
        trace.push(`status.recording:${recording}`);
      },
    };
    const stt = setupStt(
      { mic, input: textareaSttTarget(textarea) },
      status,
      () => null,
      capture,
      realtime,
    );

    mic.click();
    for (
      let turn = 0;
      turn < 4 && trace.at(-1) !== "status.local:Listening...";
      turn++
    ) {
      await Promise.resolve();
    }
    trace.length = 0;
    const sentBeforeLoss = structuredClone(socket.sent);

    socket.close();

    assert.deepEqual(trace, [
      "capture.clear",
      "capture.stop",
      "status.recording:false",
      "status.local:Dictation is temporarily unavailable. Try again.",
    ]);
    assert.deepEqual(
      socket.sent,
      sentBeforeLoss,
      "connection loss cannot emit a clear on the already unavailable socket",
    );

    stt.dispose();
    capture.dispose();
    realtime.dispose();
  } finally {
    globalThis.Event = previousEvent;
    dom.window.close();
  }
});

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
  const events = [];
  const errors = [];
  service.onState((value) => states.push(value));
  service.onEvent((value) => events.push(value));
  service.onError((value) => errors.push(value));
  for (const legacyCallback of [
    "onCommitted",
    "onSnapshot",
    "onCompleted",
    "onFailed",
  ]) {
    assert.equal(
      legacyCallback in service,
      false,
      `${legacyCallback} cannot retain callback-owned take state`,
    );
  }

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
  assert.deepEqual(
    events.map((event) => event.type),
    [
      "session.created",
      "session.updated",
      "input_audio_buffer.committed",
      "conversation.item.input_audio_transcription.hypothesis",
      "conversation.item.input_audio_transcription.completed",
      "conversation.item.input_audio_transcription.failed",
      "error",
    ],
    "the production service publishes strict decoded events for reducer ownership",
  );
  assert.deepEqual(
    errors,
    [],
    "decoded server failures publish only through the reducer event seam",
  );

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
  const events = [];
  const errors = [];
  service.onEvent((value) => events.push(value));
  service.onError((value) => errors.push(value));

  sockets[0].open();
  sockets[0].message(server.session_created);
  sockets[0].message(server.session_updated);
  events.length = 0;
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

  assert.deepEqual(events, []);
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
    const errors = [];
    const events = [];
    service.onEvent((value) => events.push(value));
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

    assert.deepEqual(
      events
        .filter(
          (event) =>
            event.type ===
            "conversation.item.input_audio_transcription.delta",
        )
        .map((event) => event.event_id),
      [
        "evt_stale_delta_1",
        "evt_stale_delta_2",
        "evt_fresh_delta_1",
        "evt_isolated_delta_1",
        "evt_fresh_delta_2",
        "evt_isolated_delta_2",
      ],
      "fallback deltas reach the reducer seam until hypotheses are negotiated",
    );
    assert.deepEqual(
      events
        .filter(
          (event) =>
            event.type ===
              "conversation.item.input_audio_transcription.hypothesis" ||
            event.type ===
              "conversation.item.input_audio_transcription.completed",
        )
        .map((event) => [event.type, event.item_id, event.transcript]),
      [
        [
          "conversation.item.input_audio_transcription.completed",
          "item_shared",
          "fresh transcript",
        ],
        [
          "conversation.item.input_audio_transcription.hypothesis",
          "item_isolated",
          "hypothesis wins",
        ],
        [
          "conversation.item.input_audio_transcription.completed",
          "item_isolated",
          "hypothesis wins",
        ],
      ],
      "strict decoded events carry every take transition without service snapshots",
    );
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
