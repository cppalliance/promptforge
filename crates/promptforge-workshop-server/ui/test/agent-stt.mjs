// Dictation on the agent session input (src/ui/agent-session-view.ts
// mounting src/ui/stt.ts), driven through the real AgentSessionService
// over a scripted wire, a scripted /stt socket, stubbed audio, and a
// recording status sink in jsdom. Pins the composer behaviors the mic
// carried before it moved here: the take is gated by the pinned wait and
// by the capability probe (a blocked click names its reason and opens no
// socket); the REC badge follows the recording; interims splice
// committed+tentative at the cursor and the final replaces them in place;
// the input is readOnly for the take's duration; a send discards the live
// take; a dying wait discards it too. Run: node test/agent-stt.mjs
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

const { window } = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
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

// Audio stubs: jsdom has no audio stack, so the getUserMedia/AudioContext
// path is scripted to succeed.
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

// A scripted /stt socket: opens asynchronously like a real one, records
// what the client sends, and lets the test push server frames.
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

// The capability probe's scripted answer: a body to serve, null to fail
// the fetch, or "pending" to hold the response until the test releases it
// through `answerPendingProbe`. Each harness sets it before the view mounts.
let capabilityAnswer = { gpu: true, engine: true };
let answerPendingProbe = null;
const probes = [];
const capabilityResponse = (body) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
globalThis.fetch = (url) => {
  probes.push(url);
  if (url !== "/stt/capability") {
    return Promise.reject(new Error(`unexpected fetch in the agent-stt test: ${url}`));
  }
  if (capabilityAnswer === null) {
    return Promise.reject(new Error("connection refused"));
  }
  if (capabilityAnswer === "pending") {
    return new Promise((resolve) => {
      answerPendingProbe = (body) => resolve(capabilityResponse(body));
    });
  }
  return Promise.resolve(capabilityResponse(capabilityAnswer));
};

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

// startStt crosses several await points (getUserMedia, socket open,
// worklet load) before sending "start"; poll until the take is live.
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

// Mounts a view over a fresh service with the probe answering
// `capability`, waits for the probe to settle, and returns the handles.
// `status` records what dictation paints: local messages and the REC state.
async function harness(capability = { gpu: true, engine: true }) {
  capabilityAnswer = capability;
  const probesBefore = probes.length;
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
  await waitFor(() => probes.length > probesBefore);
  // The probe's then-callback lands a tick after the response resolves.
  await sleep(10);
  const mic = view.element.querySelector(".agent-session__mic");
  const input = view.element.querySelector(".agent-session__input");
  const form = view.element.querySelector(".agent-session__form");
  // Clicks the mic and waits for the take's /stt socket to open and
  // send "start"; null when no take began within the wait.
  async function startTake() {
    const before = sockets.length;
    mic.click();
    const started = await waitFor(
      () => sockets.length > before && sockets.at(-1).sent.includes("start"),
    );
    return started ? sockets.at(-1) : null;
  }
  const dispose = () => {
    view.dispose();
    service.dispose();
    view.element.remove();
  };
  return { wire, service, view, status, mic, input, form, startTake, dispose };
}

await assertNoLeaks(lifecycle, async () => {
  // --- The pinned wait gates the mic; a dying wait discards the take -------

  {
    const { wire, status, mic, input, startTake, dispose } = await harness();
    check("the mic mounts enabled beside a disabled input", !mic.disabled && input.disabled);
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
    check("a gated click leaves the input disabled and unlocked", input.disabled && !input.readOnly);

    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    check("the mic click opens a /stt socket once a wait is pinned", socket !== null);
    if (socket === null) {
      dispose();
      return;
    }
    check("a live take lights the REC badge and presses the mic", status.recording && mic.getAttribute("aria-pressed") === "true");
    socket.message({ type: "interim", committed: "hello", tentative: "" });
    check("the interim lands in the pinned input", input.value === "hello" && input.readOnly);

    wire.fire.inputCancelled("tok1");
    check("a cancelled wait clears the REC badge", !status.recording);
    check("a cancelled wait closes the take's /stt socket", socket.closed);
    check("a cancelled wait lifts readOnly and drops the interim", !input.readOnly && input.value === "");
    check("a cancelled wait disables the input again", input.disabled);
    socket.message({ type: "final", text: "LATE FINAL" });
    check("a final arriving after the discard writes nothing", input.value === "");

    wire.fire.inputRequired("tok2");
    const reopened = await startTake();
    check("a fresh wait lets the mic start a fresh take", reopened !== null);
    reopened?.close();
    check("a dropped /stt socket clears the REC badge", !status.recording);

    // A new session resets the pin: the take dies with it.
    wire.fire.inputRequired("tok3");
    const third = await startTake();
    check("a take starts against the third wait", third !== null);
    wire.fire.session("s2");
    check("a new session discards the live take", third?.closed === true && !status.recording && !input.readOnly);

    dispose();
    const before = sockets.length;
    mic.click();
    await sleep(20);
    check("a click on the disposed view's mic starts nothing", sockets.length === before);
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
    check("committed and tentative join with a space", input.value === "One two. three");
    interim("One two. three four.", "");
    check("a grown committed prefix lands verbatim", input.value === "One two. three four.");
    const grownLength = input.value.length;
    interim("One two. three four. five six.", "se");
    check(
      "a shorter tentative never shrinks the text while committed grows",
      input.value === "One two. three four. five six. se" && input.value.length > grownLength,
    );
    interim("One two. three four. five six. ", "seven");
    check(
      "a trailing-whitespace committed prefix gains no double space",
      input.value === "One two. three four. five six. seven",
    );
    interim("", "fresh start");
    check("an empty committed prefix gains no leading space", input.value === "fresh start");
    dispose();
  }

  // --- Takes insert at the cursor -------------------------------------------

  {
    const { wire, input, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    input.value = "ab";
    input.setSelectionRange(1, 1);
    let socket = await startTake();
    if (socket === null) {
      failures.push("cursor insert: the mic click did not open a /stt socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "X", tentative: "" });
    check("an interim inserts at the cursor", input.value === "aXb");
    check("the cursor sits after the inserted interim", input.selectionStart === 2 && input.selectionEnd === 2);
    socket.message({ type: "final", text: "Y" });
    check("the final replaces the interim in place", input.value === "aYb" && !input.readOnly);
    check("the final closes the take's socket", socket.closed);

    input.value = "ab";
    input.setSelectionRange(0, 2);
    socket = await startTake();
    socket?.message({ type: "interim", committed: "X", tentative: "" });
    check("a selection is replaced outright", input.value === "X");
    socket?.close();

    input.value = "start";
    input.setSelectionRange(5, 5);
    socket = await startTake();
    socket?.message({ type: "final", text: " hello" });
    check("the first take appends at the end", input.value === "start hello");
    socket = await startTake();
    socket?.message({ type: "final", text: " world" });
    check(
      "a second take composes at the cursor the first left behind",
      input.value === "start hello world" && !input.readOnly,
    );
    dispose();
  }

  // --- The input is readOnly for the take's duration -------------------------

  {
    const { wire, input, mic, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok");
    input.value = "prefix";
    input.setSelectionRange(6, 6);
    const socket = await startTake();
    if (socket === null) {
      failures.push("readonly take: the mic click did not open a /stt socket");
      dispose();
      return;
    }
    check("the input is readOnly while the take is live", input.readOnly);
    check("the take marks the input as recording", input.classList.contains("stt-input--recording"));
    socket.message({ type: "interim", committed: " world", tentative: "" });
    check("the interim still lands programmatically", input.value === "prefix world");
    // Stopping through the mic sends "stop" and waits for the final.
    mic.click();
    check("a second mic click sends stop", socket.sent.includes("stop"));
    check("readOnly holds until the final arrives", input.readOnly);
    socket.message({ type: "final", text: " world" });
    check("the final lifts readOnly", !input.readOnly && !input.classList.contains("stt-input--recording"));
    check("the final text stays in place", input.value === "prefix world");
    dispose();
  }

  // --- A stopped take awaiting its final is still a take -------------------

  {
    const { wire, status, mic, input, form, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok1");
    let socket = await startTake();
    if (socket === null) {
      failures.push("stop window: the mic click did not open a /stt socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "hello", tentative: "" });
    mic.click();
    check("the stop clears the REC badge while the final is awaited", !status.recording && input.readOnly);
    wire.fire.inputCancelled("tok1");
    check("a wait dying in the stop window closes the awaited socket", socket.closed);
    check("a wait dying in the stop window lifts readOnly and drops the interim", !input.readOnly && input.value === "");
    socket.message({ type: "final", text: "LATE FINAL" });
    check("a final after a stop-window discard writes nothing", input.value === "");

    wire.fire.inputRequired("tok2");
    socket = await startTake();
    socket?.message({ type: "interim", committed: "sent as shown", tentative: "" });
    mic.click();
    form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
    check(
      "a send in the stop window carries the interim and closes the awaited socket",
      isDeepStrictEqual(wire.responses, [["tok2", "sent as shown"]]) && socket?.closed === true,
    );
    check("a send in the stop window lifts readOnly and clears the box", !input.readOnly && input.value === "");
    socket?.message({ type: "final", text: "LATE FINAL" });
    check("a final after a stop-window send writes nothing", input.value === "");

    wire.fire.inputRequired("tok3");
    input.value = "typed ";
    input.setSelectionRange(6, 6);
    socket = await startTake();
    socket?.message({ type: "interim", committed: "lost", tentative: "" });
    mic.click();
    socket?.close();
    check(
      "a socket dropping in the stop window lifts readOnly and reverts to the pre-take text",
      !input.readOnly && input.value === "typed ",
    );
    check(
      "a socket dropping in the stop window says so on the status bar",
      status.local.some((entry) => entry.label.includes("before the final transcript") && entry.severity === "error"),
    );
    dispose();
  }

  // --- A send discards the live take -----------------------------------------

  {
    const { wire, status, input, form, startTake, dispose } = await harness();
    wire.fire.inputRequired("tok1");
    const socket = await startTake();
    if (socket === null) {
      failures.push("discard on send: the mic click did not open a /stt socket");
      dispose();
      return;
    }
    socket.message({ type: "interim", committed: "hello", tentative: "" });
    check("the REC badge is lit before the send", status.recording);
    form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
    check("the send carries the interim the operator saw", isDeepStrictEqual(wire.responses, [["tok1", "hello"]]));
    check("the send clears the REC badge", !status.recording);
    check("the send closes the take's /stt socket", socket.closed);
    check("the send lifts readOnly and clears the box", !input.readOnly && input.value === "");
    socket.message({ type: "final", text: "LATE FINAL" });
    check("a late final after the send writes nothing", input.value === "");

    // Enter sends the same way the form does.
    wire.fire.inputRequired("tok2");
    const second = await startTake();
    second?.message({ type: "interim", committed: "via enter", tentative: "" });
    input.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
    check(
      "Enter during a take discards it and sends the interim",
      isDeepStrictEqual(wire.responses[1], ["tok2", "via enter"]) && second?.closed === true && !status.recording,
    );
    dispose();
  }

  // --- The capability probe gates the mic ------------------------------------

  for (const [capability, expected, name] of [
    [{ gpu: false, engine: true }, "needs a GPU", "no GPU"],
    [{ gpu: true, engine: false }, "No speech models", "no engine"],
    [null, "capability probe failed", "a failed probe"],
  ]) {
    const { wire, status, startTake, dispose } = await harness(capability);
    wire.fire.inputRequired("tok");
    const socket = await startTake();
    check(`${name} blocks the take with no /stt socket`, socket === null);
    check(
      `${name} names its reason on the status bar`,
      status.local.length === 1 && status.local[0].label.includes(expected) && status.local[0].severity === "info",
    );
    dispose();
  }

  // A click that beats the probe is refused, not let through on the wait
  // alone: a server with no engine still accepts /stt, so the gate must
  // hold until the answer is known. Once it arrives, the same click starts a take.
  {
    const { wire, status, startTake, dispose } = await harness("pending");
    wire.fire.inputRequired("tok");
    const early = await startTake();
    check("a click while the probe is in flight opens no /stt socket", early === null);
    check(
      "a click while the probe is in flight says the check is still running",
      status.local.length === 1 && status.local[0].label.includes("still checking") && status.local[0].severity === "info",
    );
    answerPendingProbe({ gpu: true, engine: true });
    await sleep(10);
    const socket = await startTake();
    check("the gate lifts once the probe answers capable", socket !== null);
    socket?.close();
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
