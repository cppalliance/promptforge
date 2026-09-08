// Dictation on the agent session input (src/ui/agent-session-view.ts
// mounting src/ui/stt.ts), driven through the real AgentSessionService
// over a scripted wire, canonical Realtime events, production capture,
// and a recording status sink in jsdom. It pins local gating and status,
// replacement snapshots, authoritative completion, overlapping items,
// clear, second take, recoverable failure, and disposal.
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const fixtureDir = path.join(
  testDir,
  "..",
  "..",
  "..",
  "gateway-stt",
  "tests",
  "fixtures",
  "realtime",
);
const canonicalSequences = JSON.parse(
  await readFile(path.join(fixtureDir, "valid-sequences.json"), "utf8"),
);

function canonicalMessage(sequence, direction, type, occurrence = 0) {
  return structuredClone(
    canonicalSequences[sequence].events.filter(
      (entry) => entry.direction === direction && entry.message.type === type,
    )[occurrence].message,
  );
}

function producerHypothesis(itemId, transcript, revision = 1) {
  return {
    type: "conversation.item.input_audio_transcription.hypothesis",
    event_id: `${itemId}_hypothesis_${revision}`,
    item_id: itemId,
    content_index: 0,
    revision,
    transcript,
    finalized: transcript,
    agreed: "",
    tentative: "",
    audio_start_ms: 0,
    audio_end_ms: 100,
  };
}

function producerCommitted(itemId) {
  return {
    type: "input_audio_buffer.committed",
    event_id: `${itemId}_committed`,
    item_id: itemId,
    previous_item_id: null,
  };
}

function producerCompletion(itemId, transcript) {
  return {
    type: "conversation.item.input_audio_transcription.completed",
    event_id: `${itemId}_completed`,
    item_id: itemId,
    content_index: 0,
    transcript,
    usage: { type: "duration", seconds: 0.1 },
  };
}

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { Emitter } from "./src/base/event.ts";
      export { AgentSessionService } from "./src/services/agent-session.ts";
      export { AgentSessionView } from "./src/ui/agent-session-view.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  loader: { ".css": "empty" },
});

// pretendToBeVisual supplies the requestAnimationFrame ProseMirror
// schedules with; the prompt input's editor mounts in every harness.
const { window } = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
});
for (const key of ["document", "HTMLElement", "Node", "Element", "KeyboardEvent"]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;
globalThis.location = window.location;
globalThis.Event = window.Event;
globalThis.KeyboardEvent = window.KeyboardEvent;
// The prompt input reads skin tokens through getComputedStyle; jsdom's
// copies must be bound to their window.
globalThis.getComputedStyle = window.getComputedStyle.bind(window);
globalThis.requestAnimationFrame = window.requestAnimationFrame.bind(window);
globalThis.cancelAnimationFrame = window.cancelAnimationFrame.bind(window);
// jsdom's Range has no layout rects; ProseMirror's scroll-to-selection
// reads them when a landed final focuses the editor.
window.Range.prototype.getClientRects = () => [];
window.Range.prototype.getBoundingClientRect = () => new window.DOMRect();

// Audio stubs: jsdom has no audio stack, so the getUserMedia/AudioContext
// path is scripted to succeed.
const fakeAudioStream = { getTracks: () => [{ stop() {} }] };
let delayedMediaStart = null;
globalThis.navigator.mediaDevices = {
  getUserMedia: () => {
    if (delayedMediaStart === null) {
      return Promise.resolve(fakeAudioStream);
    }
    delayedMediaStart.markRequested();
    return delayedMediaStart.stream;
  },
};
function delayNextMediaStart() {
  let resolveStream;
  let markRequested;
  const requested = new Promise((resolve) => {
    markRequested = resolve;
  });
  const stream = new Promise((resolve) => {
    resolveStream = resolve;
  });
  const delayed = {
    markRequested,
    requested,
    stream,
    release() {
      if (delayedMediaStart === delayed) {
        delayedMediaStart = null;
      }
      resolveStream(fakeAudioStream);
    },
  };
  delayedMediaStart = delayed;
  return delayed;
}
class FakeAudioContext {
  constructor() {
    this.sampleRate = 24_000;
    this.destination = {};
    this.audioWorklet = { addModule: () => Promise.resolve() };
  }
  createMediaStreamSource() {
    return { connect() {}, disconnect() {} };
  }
  close() {
    return Promise.resolve();
  }
  resume() {
    return Promise.resolve();
  }
}
let nextFlushAudio = null;
class FakeAudioWorkletNode {
  constructor() {
    this.port = {
      onmessage: null,
      postMessage: (message) => {
        if (message?.type === "flush") {
          const audio = nextFlushAudio;
          nextFlushAudio = null;
          queueMicrotask(() => {
            if (audio !== null) {
              this.port.onmessage?.({ data: audio });
            }
            this.port.onmessage?.({ data: { type: "flushed" } });
          });
        }
      },
    };
  }
  connect() {}
  disconnect() {}
}
window.AudioContext = FakeAudioContext;
globalThis.AudioContext = FakeAudioContext;
globalThis.AudioWorkletNode = FakeAudioWorkletNode;

// A scripted Realtime socket: opens asynchronously like a real one, records
// what the client sends, and lets the test push server frames.
const sockets = [];
let nextItem = 0;
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
    this.sent.push(JSON.parse(data));
  }
  close() {
    if (this.closed) return;
    this.closed = true;
    this.readyState = FakeWebSocket.CLOSED;
    this.dispatch("close", {});
  }
  // Test-side control, not part of the WebSocket surface.
  message(frame) {
    if (frame.type === "interim") {
      if (!this.itemId) {
        this.itemId = `item_${++nextItem}`;
      }
      const finalized = frame.committed ?? "";
      const tentative = `${finalized && frame.tentative && !/\s$/.test(finalized) ? " " : ""}${frame.tentative ?? ""}`;
      frame = {
        type: "conversation.item.input_audio_transcription.hypothesis",
        event_id: `hypothesis_${nextItem}`,
        item_id: this.itemId,
        content_index: 0,
        revision: 1,
        transcript: `${finalized}${tentative}`,
        finalized,
        agreed: "",
        tentative,
        audio_start_ms: 0,
        audio_end_ms: 100,
      };
    } else if (frame.type === "final") {
      if (!this.itemId) {
        this.itemId = `item_${++nextItem}`;
      }
      this.dispatch("message", {
        data: JSON.stringify({
          type: "input_audio_buffer.committed",
          event_id: `committed_${nextItem}`,
          item_id: this.itemId,
          previous_item_id: null,
        }),
      });
      frame = {
        type: "conversation.item.input_audio_transcription.completed",
        event_id: `completed_${nextItem}`,
        item_id: this.itemId,
        content_index: 0,
        transcript: frame.text,
        usage: { type: "duration", seconds: 0.1 },
      };
    }
    this.dispatch("message", { data: JSON.stringify(frame) });
    if (frame.type === "conversation.item.input_audio_transcription.completed") {
      this.itemId = null;
    }
  }
}
window.WebSocket = FakeWebSocket;
globalThis.WebSocket = FakeWebSocket;

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-stt-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, Emitter, AgentSessionService, AgentSessionView } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Capture crosses several await points before the take is live.
async function waitFor(condition) {
  for (let attempt = 0; attempt < 50; attempt++) {
    if (condition()) return true;
    await sleep(5);
  }
  return false;
}

// The scripted wire behind the real service: only the frames dictation cares
// about are driven (input_required, input_cancelled, agent_session).
function makeWire() {
  const emitters = {
    agents: new Emitter(),
    session: new Emitter(),
    event: new Emitter(),
    delta: new Emitter(),
    inputRequired: new Emitter(),
    inputCancelled: new Emitter(),
    error: new Emitter(),
  };
  return {
    onAgents: emitters.agents.event,
    onSession: emitters.session.event,
    onEvent: emitters.event.event,
    onDelta: emitters.delta.event,
    onInputRequired: emitters.inputRequired.event,
    onInputCancelled: emitters.inputCancelled.event,
    onError: emitters.error.event,
    responses: [],
    launch() {
      return true;
    },
    respond(token, text) {
      this.responses.push([token, text]);
      return true;
    },
    fire: {
      inputRequired: (token) => emitters.inputRequired.fire(token),
      inputCancelled: (token) => emitters.inputCancelled.fire(token),
      session: (session) => emitters.session.fire({ type: "agent_session", session, agent: "chat" }),
    },
  };
}

// Mounts a view over a fresh service and negotiated Realtime socket.
async function harness() {
  const status = {
    local: [],
    recording: false,
    showLocal(label, severity) {
      this.local.push({ label, severity });
    },
    setRecording(on) {
      this.recording = on;
    },
  };
  const wire = makeWire();
  const service = new AgentSessionService(wire);
  const view = new AgentSessionView(service, status);
  window.document.body.appendChild(view.element);
  await waitFor(() =>
    sockets.some(
      (socket) =>
        socket.url.endsWith("/v1/realtime") && socket.readyState === FakeWebSocket.OPEN,
    ),
  );
  const realtime = sockets.filter((socket) => socket.url.endsWith("/v1/realtime")).at(-1);
  realtime.message(
    canonicalMessage("first_event_readiness", "server", "session.created"),
  );
  await waitFor(() => realtime.sent.some((event) => event.type === "session.update"));
  realtime.message(
    canonicalMessage("hypothesis_negotiation", "server", "session.updated"),
  );
  const mic = view.element.querySelector(".agent-session__mic");
  // The ProseMirror prompt box: content and selection are driven through
  // the component (the DOM alone sets neither). The pending-wait gate
  // and a take's read-only both show on the editor's contenteditable
  // attribute; the take alone marks the frame with stt-input--recording.
  const input = view.promptInput;
  const editorEl = view.element.querySelector(".prompt-input__editor");
  const editable = () => editorEl.getAttribute("contenteditable") === "true";
  const recording = () => input.element.classList.contains("stt-input--recording");
  const send = view.element.querySelector(".agent-session__send");
  // Clicks the mic and waits for the take's Realtime socket to open and
  // send "start"; null when no take began within the wait.
  async function startTake() {
    mic.click();
    const started = await waitFor(() => status.recording);
    return started
      ? sockets.filter((socket) => socket.url.endsWith("/v1/realtime")).at(-1)
      : null;
  }
  const dispose = () => {
    view.dispose();
    service.dispose();
    view.element.remove();
  };
  return { wire, service, view, status, mic, input, editorEl, editable, recording, send, startTake, dispose };
}

await assertNoLeaks(lifecycle, async () => {
  // The shared canonical sequence drives fake media through UI replacement,
  // completion, second-take clear, local status, and capture cleanup.

  {
    const { wire, status, mic, input, editable, startTake, dispose } = await harness();
    wire.fire.inputRequired("fixture");
    const socket = await startTake();
    if (socket === null) {
      failures.push("canonical fixture: the first take did not start");
      dispose();
      return;
    }
    const canonicalAppend = canonicalMessage(
      "immediate_commit_and_provisional_promotion",
      "client",
      "input_audio_buffer.append",
    );
    nextFlushAudio = Uint8Array.from(
      Buffer.from(canonicalAppend.audio, "base64"),
    ).buffer;
    mic.click();
    await waitFor(() =>
      socket.sent.some((event) => event.type === "input_audio_buffer.commit"),
    );
    const append = socket.sent.find(
      (event) => event.type === "input_audio_buffer.append",
    );
    check(
      "canonical fixture emits the shared valid-sized audio append",
      append?.audio === canonicalAppend.audio,
    );
    const firstHypothesis = canonicalMessage(
      "hypothesis_negotiation",
      "server",
      "conversation.item.input_audio_transcription.hypothesis",
    );
    socket.message(firstHypothesis);
    check(
      "the first precommit hypothesis binds and replaces the active take",
      input.getText() === "Hello",
    );
    socket.message(
      canonicalMessage(
        "hypothesis_negotiation",
        "server",
        "conversation.item.input_audio_transcription.hypothesis",
        1,
      ),
    );
    check(
      "every precommit revision replaces rather than appends",
      input.getText() === "Hello!",
    );
    const committed = canonicalMessage(
      "immediate_commit_and_provisional_promotion",
      "server",
      "input_audio_buffer.committed",
    );
    committed.item_id = firstHypothesis.item_id;
    socket.message(committed);
    const beforeUnknown = input.getText();
    check(
      "the matching acknowledgment confirms without changing provisional text",
      input.getText() === "Hello!",
    );
    socket.message(
      {
        ...firstHypothesis,
        event_id: "unknown_hypothesis_after_binding",
        item_id: "unknown_item",
        transcript: "MUST NOT LAND",
      },
    );
    check(
      "an unknown hypothesis cannot replace a bound take",
      input.getText() === beforeUnknown,
    );
    socket.message(
      canonicalMessage(
        "hypothesis_negotiation",
        "server",
        "conversation.item.input_audio_transcription.completed",
      ),
    );
    check(
      "canonical completion is authoritative and restores ready UI state",
      input.getText() === "Hello" &&
        editable() &&
        status.local.at(-1).label === "Dictation ready.",
    );
    socket.message({
      ...firstHypothesis,
      event_id: "unknown_hypothesis_without_active_take",
      item_id: "orphan_item",
      transcript: "ORPHAN",
    });
    check(
      "an unknown hypothesis with no active take changes no text",
      input.getText() === "Hello",
    );

    const second = await startTake();
    check("a second take starts on the reusable fixture socket", second === socket);
    wire.fire.inputCancelled("fixture");
    const clear = socket.sent
      .filter((event) => event.type === "input_audio_buffer.clear")
      .at(-1);
    check(
      "second-take cleanup sends the canonical clear event",
      clear?.type ===
        canonicalMessage(
          "clear_retires_only_uncommitted_input",
          "client",
          "input_audio_buffer.clear",
        ).type,
    );
    check("second-take cleanup stops recording", !status.recording);
    check("second-take cleanup preserves completed text", input.getText() === "Hello");
    check("second-take cleanup restores the no-wait disabled UI state", !editable());
    dispose();
  }

  // A mismatched acknowledgment retires its provisional take and unblocks FIFO.

  {
    const { wire, status, mic, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    const socket = await startTake();
    if (socket === null) {
      failures.push("mismatch recovery: the first take did not start");
      dispose();
      return;
    }
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "mismatch_hypothesis",
      item_id: "provisional_item",
      content_index: 0,
      revision: 1,
      transcript: "must roll back",
      finalized: "",
      agreed: "",
      tentative: "must roll back",
      audio_start_ms: 0,
      audio_end_ms: 100,
    });
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 1,
    );
    socket.message({
      type: "input_audio_buffer.committed",
      event_id: "mismatched_commit",
      item_id: "wrong_item",
      previous_item_id: null,
    });
    check(
      "a mismatched acknowledgment rolls back its provisional take",
      input.getText() === "" &&
        status.local.at(-1).severity === "error" &&
        status.local.at(-1).label.includes("temporarily unavailable"),
    );

    await startTake();
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "fresh_hypothesis",
      item_id: "fresh_item",
      content_index: 0,
      revision: 1,
      transcript: "fresh take",
      finalized: "fresh",
      agreed: "",
      tentative: " take",
      audio_start_ms: 100,
      audio_end_ms: 200,
    });
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 2,
    );
    socket.message({
      type: "input_audio_buffer.committed",
      event_id: "fresh_commit",
      item_id: "fresh_item",
      previous_item_id: "wrong_item",
    });
    socket.message({
      type: "conversation.item.input_audio_transcription.completed",
      event_id: "fresh_completed",
      item_id: "fresh_item",
      content_index: 0,
      transcript: "fresh final",
      usage: { type: "duration", seconds: 0.1 },
    });
    check(
      "one mismatch cannot block the next take's matching acknowledgment",
      input.getText() === "fresh final" &&
        status.local.at(-1).label === "Dictation ready.",
    );
    dispose();
  }

  // --- The pinned wait gates the mic; a dying wait discards the take -------

  {
    const { wire, status, mic, input, editable, recording, startTake, dispose } = await harness();
    check("the mic mounts enabled beside a disabled input", !mic.disabled && !editable());
    check(
      "the mic is a push-to-talk button with an accessible name",
      mic.type === "button" &&
        mic.getAttribute("aria-label") === "Push to talk" &&
        mic.getAttribute("aria-pressed") === "false" &&
        mic.querySelector("svg") !== null,
    );
    const gated = await startTake();
    check("a mic click with no wait pinned opens no Realtime socket", gated === null);
    check(
      "a gated click names the missing wait on the status bar",
      status.local.length === 1 &&
        status.local[0].label.includes("isn't asking for input") &&
        status.local[0].severity === "info",
    );
    check("a gated click leaves the input disabled and unlocked", !editable() && !recording());

    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    check("the mic click opens a Realtime socket once a wait is pinned", socket !== null);
    if (socket === null) {
      dispose();
      return;
    }
    check("a live take lights the recording LED and presses the mic", status.recording && mic.getAttribute("aria-pressed") === "true");
    socket.message({ type: "interim", committed: "hello", tentative: "" });
    check(
      "the interim lands in the pinned input",
      input.getText() === "hello" && !editable() && recording(),
    );

    wire.fire.inputCancelled("tok1");
    check("a cancelled wait dims the recording LED", !status.recording);
    check("a cancelled wait keeps the reusable Realtime socket open", !socket.closed);
    check(
      "a cancelled wait lifts the take lock and drops the interim",
      !recording() && input.getText() === "",
    );
    check("a cancelled wait disables the input again", !editable());
    socket.message({ type: "final", text: "LATE FINAL" });
    check("a final arriving after the discard writes nothing", input.getText() === "");

    wire.fire.inputRequired("tok2");
    const reopened = await startTake();
    check("a fresh wait lets the mic start a fresh take", reopened !== null);
    wire.fire.inputCancelled("tok2");
    check("clearing the second take dims the recording LED", !status.recording);

    // A new session resets the pin: the take dies with it.
    wire.fire.inputRequired("tok3");
    const third = await startTake();
    check("a take starts against the third wait", third !== null);
    wire.fire.session("s2");
    check("a new session discards the live take", third?.closed === false && !status.recording && !recording());

    dispose();
    const before = sockets.length;
    mic.click();
    await sleep(20);
    check("a click on the disposed view's mic starts nothing", sockets.length === before);
  }

  // --- A wait swapped mid-take holds the take's lock -------------------------

  {
    const { wire, input, editable, recording, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    if (socket === null) {
      failures.push("wait swap: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "held", tentative: "" });
    wire.fire.inputRequired("tok2");
    check(
      "a wait swapped mid-take keeps the input locked to the take",
      !editable() && recording(),
    );
    socket.message({ type: "final", text: "held final" });
    check(
      "the take finishes against the new wait",
      editable() && !recording() && input.getText() === "held final",
    );
    dispose();
  }

  // --- Interims splice committed and tentative -------------------------------

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    const socket = await startTake();
    if (socket === null) {
      failures.push("interim splice: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    const interim = (committed, tentative) => socket.message({ type: "interim", committed, tentative });
    interim("One two.", "three");
    check("committed and tentative join with a space", input.getText() === "One two. three");
    interim("One two. three four.", "");
    check("a grown committed prefix lands verbatim", input.getText() === "One two. three four.");
    const grownLength = input.getText().length;
    interim("One two. three four. five six.", "se");
    check(
      "a shorter tentative never shrinks the text while committed grows",
      input.getText() === "One two. three four. five six. se" && input.getText().length > grownLength,
    );
    interim("One two. three four. five six. ", "seven");
    check(
      "a trailing-whitespace committed prefix gains no double space",
      input.getText() === "One two. three four. five six. seven",
    );
    interim("", "fresh start");
    check("an empty committed prefix gains no leading space", input.getText() === "fresh start");
    dispose();
  }

  // Producer-generated ownership snapshots replay through replacement verbatim.

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("producer");
    const socket = await startTake();
    if (socket === null) {
      failures.push("producer replay: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    const first = canonicalMessage(
      "producer_hypothesis_ownership",
      "server",
      "conversation.item.input_audio_transcription.hypothesis",
    );
    const second = canonicalMessage(
      "producer_hypothesis_ownership",
      "server",
      "conversation.item.input_audio_transcription.hypothesis",
      1,
    );
    socket.message(first);
    check(
      "producer ownership replay lands one exact transcript without a duplicated prefix",
      input.getText() === "ask not your country new tail first",
    );
    socket.message(second);
    check(
      "producer ownership revision preserves exact spaces while replacing",
      input.getText() === "ask not your country new tail second",
    );
    dispose();
  }

  // Standalone producer transcripts compose only at the logical document end.

  {
    const { wire, mic, input, editable, startTake, dispose } = await harness();
    wire.fire.inputRequired("sequential");
    const socket = await startTake();
    if (socket === null) {
      failures.push("sequential composition: the first take did not start");
      dispose();
      return;
    }
    socket.message(producerHypothesis("composition_first", "First test alpha"));
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 1,
    );
    socket.message(producerCommitted("composition_first"));
    socket.message(producerCompletion("composition_first", "First test alpha"));

    await startTake();
    socket.message(producerHypothesis("composition_second", "Second test beta"));
    check(
      "a second standalone producer hypothesis composes after the first take",
      input.getText() === "First test alpha Second test beta",
    );
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 2,
    );
    socket.message(producerCommitted("composition_second"));
    socket.message(producerCompletion("composition_second", "Second test beta"));
    check(
      "the standalone completion replaces its hypothesis without losing composition spacing",
      input.getText() === "First test alpha Second test beta" && editable(),
    );
    dispose();
  }

  {
    const { wire, mic, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("completion-only");
    input.setText("First test alpha");
    const socket = await startTake();
    if (socket === null) {
      failures.push("completion-only composition: the take did not start");
      dispose();
      return;
    }
    mic.click();
    await waitFor(() =>
      socket.sent.some((event) => event.type === "input_audio_buffer.commit"),
    );
    socket.message(producerCommitted("completion_only_second"));
    socket.message(producerCompletion("completion_only_second", "Second test beta"));
    check(
      "a completion with no hypothesis composes at the logical document end",
      input.getText() === "First test alpha Second test beta",
    );
    dispose();
  }

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("existing-space");
    input.setText("First test alpha ");
    const socket = await startTake();
    socket?.message(producerHypothesis("existing_space", "Second test beta"));
    check(
      "existing trailing space prevents an added composition separator",
      input.getText() === "First test alpha Second test beta",
    );
    dispose();
  }

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("producer-space");
    input.setText("First test alpha");
    const socket = await startTake();
    socket?.message(producerHypothesis("producer_space", " Second test beta"));
    check(
      "producer-leading space prevents a duplicate composition separator",
      input.getText() === "First test alpha Second test beta",
    );
    dispose();
  }

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("selection");
    input.setText("First test alpha");
    input.setSelection(7, 11);
    const socket = await startTake();
    socket?.message(producerHypothesis("selection", "Second"));
    check(
      "a selected replacement receives no composition separator",
      input.getText() === "First Second alpha",
    );
    dispose();
  }

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("mid-word");
    input.setText("alphaBeta");
    input.setSelection(6, 6);
    const socket = await startTake();
    socket?.message(producerHypothesis("mid_word", "Second"));
    check(
      "a mid-word insertion receives no composition separator",
      input.getText() === "alphaSecondBeta",
    );
    dispose();
  }

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("rollback-spacing");
    input.setText("First test alpha");
    const socket = await startTake();
    socket?.message(producerHypothesis("rollback_spacing", "Second test beta"));
    wire.fire.inputCancelled("rollback-spacing");
    check(
      "rolling back a composed hypothesis removes its owned separator",
      input.getText() === "First test alpha",
    );
    dispose();
  }

  // A take owns the selection present when delayed capture becomes usable.

  {
    const { wire, status, mic, input, editable, recording, dispose } = await harness();
    wire.fire.inputRequired("delayed-start");
    input.setText("old target keep");
    input.setSelection(5, 11);
    const delayed = delayNextMediaStart();
    mic.click();
    await delayed.requested;
    check("the prompt remains editable during microphone startup", editable() && !recording());

    input.setText("edited live tail");
    input.setSelection(8, 12);
    delayed.release();
    const started = await waitFor(() => status.recording);
    const socket = sockets.filter((candidate) => candidate.url.endsWith("/v1/realtime")).at(-1);
    check("delayed microphone startup completes", started);
    socket.message(producerHypothesis("delayed_prompt", "spoken"));
    check(
      "a delayed take inserts at the selection current when startup succeeds",
      input.getText() === "edited spoken tail",
    );
    wire.fire.inputCancelled("delayed-start");
    check(
      "delayed prompt rollback preserves edits made during startup",
      input.getText() === "edited live tail" && !recording(),
    );
    dispose();
  }

  // --- Takes insert at the cursor -------------------------------------------

  {
    const { wire, input, editable, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    // ProseMirror positions: inside the first paragraph, text offset + 1.
    input.setText("ab");
    input.setSelection(2, 2);
    let socket = await startTake();
    if (socket === null) {
      failures.push("cursor insert: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "X", tentative: "" });
    check("an interim inserts at the cursor", input.getText() === "aXb");
    const afterInterim = input.insertionContext().range;
    check(
      "the cursor sits after the inserted interim",
      afterInterim.start === 3 && afterInterim.end === 3,
    );
    socket.message({ type: "final", text: "Y" });
    check("the final replaces the interim in place", input.getText() === "aYb" && editable());
    check("the final keeps the reusable Realtime socket open", !socket.closed);

    input.setText("ab");
    input.setSelection(1, 3);
    socket = await startTake();
    socket?.message({ type: "interim", committed: "X", tentative: "" });
    check("a selection is replaced outright", input.getText() === "X");
    socket?.message({ type: "final", text: "X" });

    input.setText("start");
    socket = await startTake();
    socket?.message({ type: "final", text: " hello" });
    check("the first take appends at the end", input.getText() === "start hello");
    socket = await startTake();
    socket?.message({ type: "final", text: " world" });
    check(
      "a second take composes at the cursor the first left behind",
      input.getText() === "start hello world" && editable(),
    );
    dispose();
  }

  // --- The input is read-only for the take's duration ------------------------

  {
    const { wire, input, mic, editable, recording, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    input.setText("prefix");
    const socket = await startTake();
    if (socket === null) {
      failures.push("readonly take: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    check("the input is read-only while the take is live", !editable());
    check("the take marks the input as recording", recording());
    socket.message({ type: "interim", committed: " world", tentative: "" });
    check("the interim still lands programmatically", input.getText() === "prefix world");
    // Stopping through the mic sends "stop" and waits for the final.
    mic.click();
    await waitFor(() => socket.sent.some((event) => event.type === "input_audio_buffer.commit"));
    check(
      "a second mic click sends the canonical commit event",
      socket.sent.some((event) => event.type === "input_audio_buffer.commit"),
    );
    check("the take lock holds until the final arrives", !editable());
    socket.message({ type: "final", text: " world" });
    check("the final lifts the take lock", editable() && !recording());
    check("the final text stays in place", input.getText() === "prefix world");
    dispose();
  }

  // --- A stopped take awaiting its final is still a take -------------------

  {
    const { wire, status, mic, input, editable, recording, send, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok1");
    let socket = await startTake();
    if (socket === null) {
      failures.push("stop window: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "hello", tentative: "" });
    mic.click();
    check("the stop dims the recording LED while the final is awaited", !status.recording && !editable());
    wire.fire.inputCancelled("tok1");
    check("a wait dying in the stop window keeps the Realtime session reusable", !socket.closed);
    check(
      "a wait dying in the stop window lifts the take lock and drops the interim",
      !recording() && input.getText() === "",
    );
    socket.message({ type: "final", text: "LATE FINAL" });
    check("a final after a stop-window discard writes nothing", input.getText() === "");

    wire.fire.inputRequired("tok2");
    socket = await startTake();
    socket?.message({ type: "interim", committed: "sent as shown", tentative: "" });
    mic.click();
    send.click();
    check(
      "a send in the stop window carries the interim",
      isDeepStrictEqual(wire.responses, [["tok2", "sent as shown"]]) && socket?.closed === false,
    );
    check(
      "a send in the stop window lifts the take lock and clears the box",
      !recording() && input.getText() === "",
    );
    socket?.message({ type: "final", text: "LATE FINAL" });
    check("a final after a stop-window send writes nothing", input.getText() === "");

    wire.fire.inputRequired("tok3");
    input.setText("typed ");
    socket = await startTake();
    socket?.message({ type: "interim", committed: "lost", tentative: "" });
    mic.click();
    socket?.close();
    check(
      "a socket dropping in the stop window lifts the take lock and reverts to the pre-take text",
      editable() && input.getText() === "typed ",
    );
    check(
      "a socket dropping in the stop window says so on the status bar",
      status.local.some((entry) => entry.label.includes("temporarily unavailable") && entry.severity === "error"),
    );
    dispose();
  }

  // A stop keeps routing the worklet's carried block until flush completes.

  {
    const { wire, mic, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    const socket = await startTake();
    if (socket === null) {
      failures.push("flush ordering: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    nextFlushAudio = Uint8Array.from([1, 0, 2, 0]).buffer;
    mic.click();
    await waitFor(() =>
      socket.sent.some((event) => event.type === "input_audio_buffer.commit"),
    );
    const speechEvents = socket.sent.filter((event) =>
      event.type.startsWith("input_audio_buffer."),
    );
    check(
      "stop sends the worklet's carried PCM block before commit",
      speechEvents.length === 2 &&
        speechEvents[0].type === "input_audio_buffer.append" &&
        speechEvents[0].audio === "AQACAA==" &&
        speechEvents[1].type === "input_audio_buffer.commit",
    );
    dispose();
  }

  // --- A send discards the live take -----------------------------------------

  {
    const { wire, status, input, editorEl, recording, send, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    if (socket === null) {
      failures.push("discard on send: the mic click did not open a Realtime socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "hello", tentative: "" });
    check("the recording LED is lit before the send", status.recording);
    send.click();
    check("the send carries the interim the operator saw", isDeepStrictEqual(wire.responses, [["tok1", "hello"]]));
    check("the send dims the recording LED", !status.recording);
    check("the send keeps the reusable Realtime socket open", !socket.closed);
    check(
      "the send lifts the take lock and clears the box",
      !recording() && input.getText() === "",
    );
    socket.message({ type: "final", text: "LATE FINAL" });
    check("a late final after the send writes nothing", input.getText() === "");

    // Enter sends the same way the button does - even mid-take, when the
    // editor is read-only and ProseMirror drops its keydown.
    wire.fire.inputRequired("tok2");
    const second = await startTake();
    second?.message({ type: "interim", committed: "via enter", tentative: "" });
    editorEl.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
    check(
      "Enter during a take discards it and sends the interim",
      isDeepStrictEqual(wire.responses[1], ["tok2", "via enter"]) && second?.closed === false && !status.recording,
    );
    dispose();
  }

  // A discarded commit keeps its FIFO place until its acknowledgment arrives.

  {
    const { wire, mic, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    if (socket === null) {
      failures.push("commit tombstone: the first take did not start");
      dispose();
      return;
    }
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 1,
    );
    wire.fire.inputCancelled("tok1");

    wire.fire.inputRequired("tok2");
    await startTake();
    socket.message({
      type: "input_audio_buffer.committed",
      event_id: "late_discarded_commit",
      item_id: "discarded_item",
      previous_item_id: null,
    });
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "late_discarded_hypothesis",
      item_id: "discarded_item",
      content_index: 0,
      revision: 1,
      transcript: "WRONG TAKE",
      finalized: "",
      agreed: "",
      tentative: "WRONG TAKE",
      audio_start_ms: 0,
      audio_end_ms: 100,
    });
    check(
      "a discarded commit's late acknowledgment and hypothesis do not bind the new take",
      input.getText() === "",
    );

    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 2,
    );
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "current_hypothesis",
      item_id: "current_item",
      content_index: 0,
      revision: 1,
      transcript: "right take",
      finalized: "right",
      agreed: "",
      tentative: " take",
      audio_start_ms: 100,
      audio_end_ms: 200,
    });
    socket.message({
      type: "input_audio_buffer.committed",
      event_id: "current_commit",
      item_id: "current_item",
      previous_item_id: "discarded_item",
    });
    check(
      "a precommit hypothesis after a tombstone binds the current take",
      input.getText() === "right take",
    );
    dispose();
  }

  // --- Overlapping items finalize independently ------------------------------

  {
    const { wire, mic, input, editable, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    input.setText("base ");
    const socket = await startTake();
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 1,
    );
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "overlap_hypothesis_a",
      item_id: "item_overlap_a",
      content_index: 0,
      revision: 1,
      transcript: "first",
      finalized: "fir",
      agreed: "s",
      tentative: "t",
      audio_start_ms: 0,
      audio_end_ms: 100,
    });
    socket.message(
      canonicalMessage(
        "overlapping_items_reverse_completion",
        "server",
        "input_audio_buffer.committed",
      ),
    );

    await startTake();
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 2,
    );
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "overlap_hypothesis_b",
      item_id: "item_overlap_b",
      content_index: 0,
      revision: 1,
      transcript: " second",
      finalized: " sec",
      agreed: "on",
      tentative: "d",
      audio_start_ms: 100,
      audio_end_ms: 200,
    });
    socket.message(
      canonicalMessage(
        "overlapping_items_reverse_completion",
        "server",
        "input_audio_buffer.committed",
        1,
      ),
    );
    check(
      "overlapping hypotheses occupy isolated replacement regions",
      input.getText() === "base first second" && !editable(),
    );

    for (const completion of [0, 1]) {
      socket.message(canonicalMessage(
        "overlapping_items_reverse_completion",
        "server",
        "conversation.item.input_audio_transcription.completed",
        completion,
      ));
    }
    check(
      "reverse completion replaces each item with authoritative text",
      input.getText() === "base first second" && editable(),
    );
    dispose();
  }

  // A correlated rejection rolls back only the client event's take.

  {
    const { wire, mic, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    const socket = await startTake();
    if (socket === null) {
      failures.push("correlated error: the first take did not start");
      dispose();
      return;
    }
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 1,
    );
    socket.message({
      type: "input_audio_buffer.committed",
      event_id: "older_commit",
      item_id: "older_item",
      previous_item_id: null,
    });
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "older_hypothesis",
      item_id: "older_item",
      content_index: 0,
      revision: 1,
      transcript: "older",
      finalized: "old",
      agreed: "",
      tentative: "er",
      audio_start_ms: 0,
      audio_end_ms: 100,
    });

    await startTake();
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 2,
    );
    const rejectedCommit = socket.sent
      .filter((event) => event.type === "input_audio_buffer.commit")
      .at(-1);
    socket.message({
      type: "error",
      event_id: "rejected_commit",
      error: {
        type: "invalid_request_error",
        code: "audio_too_short",
        message: "SERVER WORDING MUST NOT LEAK",
        param: "audio",
        event_id: rejectedCommit.event_id,
      },
    });
    check(
      "a commit rejection rolls back its take but preserves an older finalization",
      typeof rejectedCommit.event_id === "string" && input.getText() === "older",
    );
    socket.message({
      type: "conversation.item.input_audio_transcription.completed",
      event_id: "older_completed",
      item_id: "older_item",
      content_index: 0,
      transcript: "OLDER FINAL",
      usage: { type: "duration", seconds: 0.1 },
    });
    check(
      "the preserved older item still accepts authoritative completion",
      input.getText() === "OLDER FINAL",
    );
    dispose();
  }

  // --- Recoverable Realtime errors use local wording -------------------------

  {
    const { wire, status, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    const socket = await startTake();
    socket?.message({ type: "interim", committed: "temporary", tentative: "" });
    socket?.message({
      type: "error",
      event_id: "server_error",
      error: {
        type: "server_error",
        code: "engine_replaced",
        message: "SERVER WORDING MUST NOT LEAK",
      },
    });
    check(
      "a recoverable server error restores the pre-take text",
      input.getText() === "",
    );
    check(
      "a recoverable server error is worded locally",
      status.local.at(-1).label.includes("temporarily unavailable") &&
        !status.local.at(-1).label.includes("SERVER WORDING"),
    );
    dispose();
  }

  // A real second socket may immediately reuse the first socket's item ID.

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("reconnect");
    const first = await startTake();
    if (first === null) {
      failures.push("two-socket reconnect: the first take did not start");
      dispose();
      return;
    }
    first.message(producerHypothesis("reused_item", "old socket words"));
    check("the first socket owns its provisional text", input.getText() === "old socket words");
    first.close();
    check("closing the first socket rolls its text back", input.getText() === "");

    await sleep(1_050);
    const secondCreated = await waitFor(
      () =>
        sockets.filter((socket) => socket.url.endsWith("/v1/realtime")).length >= 2 &&
        sockets.at(-1) !== first,
    );
    const second = sockets.at(-1);
    check("the reconnect creates a distinct second socket", secondCreated && second !== first);
    await waitFor(() => second.readyState === FakeWebSocket.OPEN);
    second.message(
      canonicalMessage("first_event_readiness", "server", "session.created"),
    );
    await waitFor(() => second.sent.some((event) => event.type === "session.update"));
    second.message(
      canonicalMessage("hypothesis_negotiation", "server", "session.updated"),
    );

    const active = await startTake();
    check("the fresh take uses the second socket", active === second);
    second.message(producerHypothesis("reused_item", "fresh socket words"));
    check(
      "the second socket immediately reuses the same item ID",
      input.getText() === "fresh socket words",
    );
    first.dispatch("message", {
      data: JSON.stringify(producerCompletion("reused_item", "STALE FINAL")),
    });
    check(
      "a late first-socket callback cannot rewrite the fresh take",
      input.getText() === "fresh socket words",
    );
    second.message(producerCompletion("reused_item", "fresh final"));
    check("the second socket completion remains authoritative", input.getText() === "fresh final");
    dispose();
  }
});

if (failures.length > 0) {
  console.error(`agent-stt: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-stt: all assertions passed");
process.exit(0);
