// The agent-session view (src/ui/agent-session-view.ts) in jsdom, driven
// through the real AgentSessionService over a scripted wire: durable
// events paint semantic feed rows (user text, model-labelled replies,
// collapsible reasoning, tool rows, tool output, error rows); streaming
// deltas paint a pending row the durable event settles, with the settled
// history never rebuilt; the input pins to the pending wait, answers it
// byte-exact, and returns to disabled. Run: node test/agent-session-view.mjs
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
  // The module under test imports its colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
const { window } = dom;
for (const key of ["document", "HTMLElement", "Node", "Element", "Event", "KeyboardEvent"]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.window = window;
globalThis.document = window.document;
globalThis.Event = window.Event;
globalThis.KeyboardEvent = window.KeyboardEvent;
// The view probes voice capability on mount; this suite is not about
// voice (test/agent-voice.mjs is), so the probe fails and the mic stays
// gated. Any other fetch is a regression.
globalThis.fetch = (url) =>
  Promise.reject(new Error(`unexpected fetch in the agent-session-view test: ${url}`));

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-session-view-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, Emitter, AgentSessionService, AgentSessionView } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The scripted wire behind the real service, so the test drives the same
// frames the socket would deliver.
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
      event: (kind, content, extra = {}) => {
        const { reply, ...eventFields } = extra;
        const frame = {
          type: "agent_event",
          index: 0,
          event: { kind, section: "chat", chain_id: 0, depth: 0, turn: 0, content, ...eventFields },
        };
        if (reply !== undefined) frame.reply = reply;
        emitters.event.fire(frame);
      },
      delta: (kind, content, reply) =>
        emitters.delta.fire({ type: "agent_delta", kind, content, reply }),
      inputRequired: (token) => emitters.inputRequired.fire(token),
      inputCancelled: (token) => emitters.inputCancelled.fire(token),
      error: (message) => emitters.error.fire(message),
      session: (session) => emitters.session.fire({ type: "agent_session", session, agent: "chat" }),
    },
  };
}

// Voice's status sink: this suite never records, so nothing lands here.
const silentStatus = { showLocal() {}, setRecording() {} };

function harness() {
  const wire = makeWire();
  const service = new AgentSessionService(wire);
  const view = new AgentSessionView(service, silentStatus);
  window.document.body.appendChild(view.element);
  const rows = () => [...view.element.querySelectorAll(".agent-item")];
  const input = view.element.querySelector(".agent-session__input");
  const send = view.element.querySelector(".agent-session__send");
  const form = view.element.querySelector(".agent-session__form");
  const dispose = () => {
    view.dispose();
    service.dispose();
    view.element.remove();
  };
  return { wire, service, view, rows, input, send, form, dispose };
}

await assertNoLeaks(lifecycle, () => {
  // --- Durable events paint semantic rows ----------------------------------

  {
    const { wire, rows, dispose } = harness();
    wire.fire.event("user_message", "hi <b>there</b>");
    wire.fire.event("agent_message", "hello back", { model: "llama-3", reply: 0 });
    wire.fire.event("agent_thought", "step one", { model: "llama-3", reply: 1 });
    wire.fire.event("tool_call", '[{"id":"call_1","name":"read","arguments":{"path":"a"}}]', {
      model: "llama-3",
      reply: 1,
    });
    wire.fire.event("tool_call_update", "the file body", { tool_call_id: "call_1" });

    const [user, reply, reasoning, toolCall, toolResult] = rows();
    check(
      "a user event paints a user row with its origin line",
      user?.classList.contains("agent-item--user") === true &&
        user.querySelector(".agent-item__meta")?.textContent === "You",
    );
    check(
      "untrusted content lands as text, never markup",
      user?.querySelector("b") === null &&
        user?.querySelector(".agent-item__text")?.textContent === "hi <b>there</b>",
    );
    check(
      "a reply row carries its model label",
      reply?.classList.contains("agent-item--reply") === true &&
        reply.querySelector(".agent-item__meta")?.textContent === "llama-3" &&
        reply.querySelector(".agent-item__text")?.textContent === "hello back",
    );
    const details = reasoning?.querySelector("details.agent-item__reasoning");
    check(
      "a thought paints a collapsible reasoning block naming its model",
      details !== null &&
        details?.querySelector("summary")?.textContent === "Reasoning (llama-3)" &&
        details?.querySelector(".agent-item__text")?.textContent === "step one",
    );
    check("a settled reasoning block is collapsed", details?.open === false);
    const callRows = [...(toolCall?.querySelectorAll(".agent-item__call") ?? [])];
    check(
      "a tool-call batch paints one row per call with name and arguments",
      callRows.length === 1 &&
        callRows[0].querySelector(".agent-item__call-name")?.textContent === "read" &&
        callRows[0].querySelector(".agent-item__call-args")?.textContent === '{"path":"a"}',
    );
    check(
      "a tool result paints its call id and preformatted output",
      toolResult?.querySelector(".agent-item__meta")?.textContent === "Tool result (call_1)" &&
        toolResult?.querySelector("pre.agent-item__output")?.textContent === "the file body",
    );
    dispose();
  }

  // --- Streaming: pending rows settle in place, history stands --------------

  {
    const { wire, rows, dispose } = harness();
    wire.fire.event("user_message", "question");
    const userRow = rows()[0];
    wire.fire.delta("reasoning", "let me ", 0);
    wire.fire.delta("reasoning", "think", 0);
    let pendingReasoning = rows()[1];
    check(
      "reasoning deltas paint one pending open block",
      rows().length === 2 &&
        pendingReasoning?.classList.contains("agent-item--pending") === true &&
        pendingReasoning.querySelector("details")?.open === true &&
        pendingReasoning.querySelector(".agent-item__text")?.textContent === "let me think",
    );
    wire.fire.event("agent_thought", "let me think", { model: "m", reply: 0 });
    wire.fire.delta("text", "the ans", 0);
    wire.fire.delta("text", "wer", 0);
    check(
      "text deltas paint one pending reply after the settled thought",
      rows().length === 3 &&
        rows()[2]?.classList.contains("agent-item--pending") === true &&
        rows()[2]?.querySelector(".agent-item__text")?.textContent === "the answer",
    );
    wire.fire.event("agent_message", "the answer", { model: "m", reply: 0 });
    check(
      "the durable reply settles the pending row",
      rows().length === 3 &&
        rows()[2]?.classList.contains("agent-item--pending") === false &&
        rows()[2]?.querySelector(".agent-item__meta")?.textContent === "m",
    );
    check(
      "settled history is never rebuilt: the user row is the same node",
      rows()[0] === userRow,
    );
    dispose();
  }

  // --- Error frames paint labelled error rows --------------------------------

  {
    const { wire, rows, dispose } = harness();
    wire.fire.error("the model call failed");
    const row = rows()[0];
    check(
      "an error paints an error row with a visible label, not color alone",
      row?.classList.contains("agent-item--error") === true &&
        row.querySelector("strong")?.textContent === "Error: " &&
        row.querySelector(".agent-item__text")?.textContent === "Error: the model call failed",
    );
    dispose();
  }

  // --- The input pins to the pending wait ------------------------------------

  {
    const { wire, input, send, form, dispose } = harness();
    check(
      "the input starts disabled with no wait open",
      input.disabled === true && send.disabled === true,
    );
    wire.fire.inputRequired("tok1");
    check(
      "an input_required enables the pinned input",
      input.disabled === false && send.disabled === false,
    );
    input.value = "two  spaces ";
    form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
    check(
      "submitting answers the wait byte-exact, untrimmed",
      isDeepStrictEqual(wire.responses, [["tok1", "two  spaces "]]),
    );
    check("a successful send clears the box", input.value === "");
    check(
      "the spent wait returns the input to disabled",
      input.disabled === true && send.disabled === true,
    );
    wire.fire.inputRequired("tok2");
    input.value = "";
    form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }));
    check("an empty box sends nothing", wire.responses.length === 1);
    input.value = "enter sends";
    input.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
    check(
      "Enter submits without a form event",
      isDeepStrictEqual(wire.responses[1], ["tok2", "enter sends"]),
    );
    wire.fire.inputRequired("tok3");
    input.value = "変換中";
    input.dispatchEvent(
      new window.KeyboardEvent("keydown", {
        key: "Enter",
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    check(
      "Enter that commits an IME composition does not submit",
      wire.responses.length === 2 && input.value === "変換中",
    );
    wire.fire.inputCancelled("tok3");
    check("a cancelled wait returns the input to disabled", input.disabled === true);
    dispose();
  }

  // --- A new session clears the feed -----------------------------------------

  {
    const { wire, rows, dispose } = harness();
    wire.fire.session("s1");
    wire.fire.event("user_message", "old");
    check("the first session's events paint", rows().length === 1);
    wire.fire.session("s2");
    check("a new session id clears the feed", rows().length === 0);
    dispose();
  }
});

if (failures.length > 0) {
  console.error(`agent-session-view: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-session-view: all assertions passed");
process.exit(0);
