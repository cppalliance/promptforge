// Dictation on the agent session input (src/ui/agent-session-view.ts
// mounting src/ui/stt.ts), driven through the real AgentSessionService
// over a scripted wire, canonical Realtime events, production capture,
// and a recording status sink in jsdom. It pins local gating and status,
// replacement snapshots, authoritative completion, overlapping items,
// clear, second take, recoverable failure, and disposal.
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

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
globalThis.navigator.mediaDevices = {
  getUserMedia: () => Promise.resolve(fakeAudioStream),
};
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

// A scripted /stt socket: opens asynchronously like a real one, records
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
    if (frame.type === "interim" || frame.type === "final") {
      if (!this.itemId) {
        this.itemId = `item_${++nextItem}`;
        this.dispatch("message", {
          data: JSON.stringify({
            type: "input_audio_buffer.committed",
            event_id: `committed_${nextItem}`,
            item_id: this.itemId,
            previous_item_id: null,
          }),
        });
      }
      frame =
        frame.type === "interim"
          ? {
              type: "conversation.item.input_audio_transcription.hypothesis",
              event_id: `hypothesis_${nextItem}`,
              item_id: this.itemId,
              content_index: 0,
              revision: 1,
              transcript: [frame.committed, frame.tentative].filter(Boolean).join(
                frame.committed && frame.tentative && !/\s$/.test(frame.committed) ? " " : "",
              ),
              finalized: frame.committed ?? "",
              agreed: "",
              tentative: frame.tentative ?? "",
              audio_start_ms: 0,
              audio_end_ms: 100,
            }
          : {
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
  realtime.message({
    type: "session.created",
    event_id: "created",
    session: { id: "session", object: "realtime.transcription_session", type: "transcription", include: [], audio: { input: {} } },
  });
  await waitFor(() => realtime.sent.some((event) => event.type === "session.update"));
  realtime.message({
    type: "session.updated",
    event_id: "updated",
    session: {
      id: "session",
      object: "realtime.transcription_session",
      type: "transcription",
      include: ["item.input_audio_transcription.hypothesis"],
      audio: { input: {} },
    },
  });
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
  // Clicks the mic and waits for the take's /stt socket to open and
  // send "start"; null when no take began within the wait.
  async function startTake() {
    mic.click();
    const started = await waitFor(() => status.recording);
    return started ? realtime : null;
  }
  const dispose = () => {
    view.dispose();
    service.dispose();
    view.element.remove();
  };
  return { wire, service, view, status, mic, input, editorEl, editable, recording, send, startTake, dispose };
}

await assertNoLeaks(lifecycle, async () => {
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
    check("a mic click with no wait pinned opens no /stt socket", gated === null);
    check(
      "a gated click names the missing wait on the status bar",
      status.local.length === 1 &&
        status.local[0].label.includes("isn't asking for input") &&
        status.local[0].severity === "info",
    );
    check("a gated click leaves the input disabled and unlocked", !editable() && !recording());

    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    check("the mic click opens a /stt socket once a wait is pinned", socket !== null);
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
      failures.push("wait swap: the mic click did not open a /stt socket");
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
      failures.push("interim splice: the mic click did not open a /stt socket");
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

  // --- Takes insert at the cursor -------------------------------------------

  {
    const { wire, input, editable, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    // ProseMirror positions: inside the first paragraph, text offset + 1.
    input.setText("ab");
    input.setSelection(2, 2);
    let socket = await startTake();
    if (socket === null) {
      failures.push("cursor insert: the mic click did not open a /stt socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "X", tentative: "" });
    check("an interim inserts at the cursor", input.getText() === "aXb");
    check(
      "the cursor sits after the inserted interim",
      input.getSelection().start === 3 && input.getSelection().end === 3,
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
      failures.push("readonly take: the mic click did not open a /stt socket");
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
      failures.push("stop window: the mic click did not open a /stt socket");
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
      failures.push("discard on send: the mic click did not open a /stt socket");
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
      type: "input_audio_buffer.committed",
      event_id: "current_commit",
      item_id: "current_item",
      previous_item_id: "discarded_item",
    });
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
    check(
      "the acknowledgment after a tombstone binds the current take",
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
      type: "input_audio_buffer.committed",
      event_id: "overlap_commit_a",
      item_id: "overlap_a",
      previous_item_id: null,
    });
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "overlap_hypothesis_a",
      item_id: "overlap_a",
      content_index: 0,
      revision: 1,
      transcript: "first",
      finalized: "fir",
      agreed: "s",
      tentative: "t",
      audio_start_ms: 0,
      audio_end_ms: 100,
    });

    await startTake();
    mic.click();
    await waitFor(
      () =>
        socket.sent.filter((event) => event.type === "input_audio_buffer.commit").length === 2,
    );
    socket.message({
      type: "input_audio_buffer.committed",
      event_id: "overlap_commit_b",
      item_id: "overlap_b",
      previous_item_id: "overlap_a",
    });
    socket.message({
      type: "conversation.item.input_audio_transcription.hypothesis",
      event_id: "overlap_hypothesis_b",
      item_id: "overlap_b",
      content_index: 0,
      revision: 1,
      transcript: " second",
      finalized: " sec",
      agreed: "on",
      tentative: "d",
      audio_start_ms: 100,
      audio_end_ms: 200,
    });
    check(
      "overlapping hypotheses occupy isolated replacement regions",
      input.getText() === "base first second" && !editable(),
    );

    for (const [itemId, transcript] of [
      ["overlap_b", " SECOND"],
      ["overlap_a", "FIRST LONG"],
    ]) {
      socket.message({
        type: "conversation.item.input_audio_transcription.completed",
        event_id: `overlap_done_${itemId}`,
        item_id: itemId,
        content_index: 0,
        transcript,
        usage: { type: "duration", seconds: 0.1 },
      });
    }
    check(
      "reverse completion replaces each item with authoritative text",
      input.getText() === "base FIRST LONG SECOND" && editable(),
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
});

if (failures.length > 0) {
  console.error(`agent-stt: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-stt: all assertions passed");
process.exit(0);
