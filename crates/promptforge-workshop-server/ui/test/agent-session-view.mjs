// The agent-session view (src/ui/agent-session-view.ts) in jsdom, driven
// through the real AgentSessionService over a scripted wire: durable
// events paint semantic feed rows (user text, model-labelled replies as
// sanitized markdown, collapsible reasoning, collapsible tool cards,
// tool output, error rows); streaming deltas paint a pending row the
// durable event settles, with the settled history never rebuilt; the
// input pins to the pending wait, answers it byte-exact, and returns to
// disabled; a view built with a ModelService mounts the toolbar (mode
// chip, model picker, context ring) between the feed and the input bar.
// Run: node test/agent-session-view.mjs
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
      export { ModelService } from "./src/services/model-service.ts";
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

// pretendToBeVisual supplies the requestAnimationFrame ProseMirror
// schedules with; the prompt input's editor mounts in every harness.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
  pretendToBeVisual: true,
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
// The toolbar's mode chip dispatches a CustomEvent on the document;
// jsdom's dispatchEvent rejects Node's realm, so the bundle needs
// jsdom's constructor.
globalThis.CustomEvent = window.CustomEvent;
// The prompt input reads skin tokens through getComputedStyle; jsdom's
// copies must be bound to their window.
globalThis.getComputedStyle = window.getComputedStyle.bind(window);
globalThis.requestAnimationFrame = window.requestAnimationFrame.bind(window);
globalThis.cancelAnimationFrame = window.cancelAnimationFrame.bind(window);
// The view probes STT capability on mount; this suite is not about
// dictation (test/agent-stt.mjs is), so the probe fails and the mic stays
// gated. Any other fetch is a regression.
globalThis.fetch = (url) =>
  Promise.reject(new Error(`unexpected fetch in the agent-session-view test: ${url}`));

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-session-view-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, Emitter, AgentSessionService, AgentSessionView, ModelService } = await import(
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

// Dictation's status sink: this suite never records, so nothing lands here.
const silentStatus = { showLocal() {}, setRecording() {} };

function harness() {
  const wire = makeWire();
  const service = new AgentSessionService(wire);
  const view = new AgentSessionView(service, silentStatus);
  window.document.body.appendChild(view.element);
  const rows = () => [...view.element.querySelectorAll(".agent-item")];
  // The ProseMirror prompt box: content and selection are driven through
  // the component (the DOM alone sets neither), and the pending-wait
  // gate shows on the editor's contenteditable attribute.
  const input = view.promptInput;
  const editorEl = view.element.querySelector(".prompt-input__editor");
  const editable = () => editorEl.getAttribute("contenteditable") === "true";
  const send = view.element.querySelector(".agent-session__send");
  const dispose = () => {
    view.dispose();
    service.dispose();
    view.element.remove();
  };
  return { wire, service, view, rows, input, editorEl, editable, send, dispose };
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
    wire.fire.event("agent_message", "**bold** `code` <script>alert(1)</script>", {
      model: "llama-3",
      reply: 2,
    });

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
      "a reply row carries its model label and renders markdown",
      reply?.classList.contains("agent-item--reply") === true &&
        reply.querySelector(".agent-item__meta")?.textContent === "llama-3" &&
        reply.querySelector(".markdown-content")?.textContent === "hello back",
    );
    const details = reasoning?.querySelector("details.agent-item__reasoning");
    check(
      "a thought paints a collapsible reasoning block naming its model",
      details !== null &&
        details?.querySelector("summary")?.textContent === "Reasoning (llama-3)" &&
        details?.querySelector(".markdown-content")?.textContent === "step one",
    );
    check("a settled reasoning block is collapsed", details?.open === false);
    const card = toolCall?.querySelector("details.tool-call-card");
    check(
      "a tool-call batch paints a card naming the tool with a call-count badge",
      card !== null &&
        card?.querySelector(".tool-call-card__name")?.textContent === "read" &&
        card?.querySelector(".tool-call-card__count")?.textContent === "1",
    );
    check(
      "the card body renders the call's arguments",
      card?.querySelector(".tool-call-card__args")?.textContent?.includes('{"path":"a"}') === true,
    );
    check(
      "a card whose result already landed starts collapsed",
      card?.open === false && card?.classList.contains("tool-call-card--running") === false,
    );
    check(
      "a tool result paints its call id and preformatted output",
      toolResult?.querySelector(".agent-item__meta")?.textContent === "Tool result (call_1)" &&
        toolResult?.querySelector("pre.agent-item__output")?.textContent === "the file body",
    );
    const markdownRow = rows()[5];
    check(
      "a reply renders its markdown formatting, not the raw source",
      markdownRow?.querySelector(".markdown-content strong")?.textContent === "bold" &&
        markdownRow?.querySelector(".markdown-content code")?.textContent === "code",
    );
    check(
      "model-authored markup is sanitized before it lands",
      markdownRow?.querySelector("script") === null,
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
        pendingReasoning.querySelector(".markdown-content")?.textContent === "let me think",
    );
    wire.fire.event("agent_thought", "let me think", { model: "m", reply: 0 });
    wire.fire.delta("text", "the ans", 0);
    wire.fire.delta("text", "wer", 0);
    check(
      "text deltas paint one pending reply after the settled thought",
      rows().length === 3 &&
        rows()[2]?.classList.contains("agent-item--pending") === true &&
        rows()[2]?.querySelector(".markdown-content")?.textContent === "the answer",
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

  // --- Tool cards open while running and collapse when the result lands ------

  {
    const { wire, rows, dispose } = harness();
    wire.fire.event("tool_call", '[{"id":"call_9","name":"read","arguments":{"path":"a"}}]', {
      model: "m",
      reply: 0,
    });
    const cardRow = rows()[0];
    const card = cardRow?.querySelector("details.tool-call-card");
    check(
      "a tool card auto-opens while its call is running",
      card?.open === true && card.classList.contains("tool-call-card--running"),
    );
    wire.fire.event("tool_call_update", "the file body", { tool_call_id: "call_9" });
    check(
      "the landing result collapses the card without rebuilding its row",
      rows()[0] === cardRow &&
        rows().length === 2 &&
        card?.open === false &&
        card.classList.contains("tool-call-card--running") === false,
    );
    check(
      "the result still paints its own row",
      rows()[1]?.querySelector("pre.agent-item__output")?.textContent === "the file body",
    );
    dispose();
  }

  // --- An unparsed tool-call batch paints its raw text in a card -------------

  {
    const { wire, rows, dispose } = harness();
    wire.fire.event("tool_call", "not json at all", { model: "m", reply: 0 });
    const card = rows()[0]?.querySelector("details.tool-call-card");
    check(
      "an unparsed batch paints a collapsed card carrying its raw text",
      card !== null &&
        card?.querySelector(".tool-call-card__raw")?.textContent === "not json at all" &&
        card?.open === false &&
        card?.classList.contains("tool-call-card--running") === false,
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
    const { wire, input, editorEl, editable, send, dispose } = harness();
    check(
      "the input starts disabled with no wait open",
      !editable() && send.disabled === true,
    );
    wire.fire.inputRequired("tok1");
    check(
      "an input_required enables the pinned input",
      editable() && send.disabled === false,
    );
    input.setText("two  spaces ");
    send.click();
    check(
      "submitting answers the wait byte-exact, untrimmed",
      isDeepStrictEqual(wire.responses, [["tok1", "two  spaces "]]),
    );
    check("a successful send clears the box", input.getText() === "");
    check(
      "the spent wait returns the input to disabled",
      !editable() && send.disabled === true,
    );
    wire.fire.inputRequired("tok2");
    send.click();
    check("an empty box sends nothing", wire.responses.length === 1);
    input.setText("enter sends");
    editorEl.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
    check(
      "Enter submits without a button click",
      isDeepStrictEqual(wire.responses[1], ["tok2", "enter sends"]),
    );
    wire.fire.inputRequired("tok3");
    input.setText("変換中");
    editorEl.dispatchEvent(
      new window.KeyboardEvent("keydown", {
        key: "Enter",
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    check(
      "Enter that commits an IME composition does not submit",
      wire.responses.length === 2 && input.getText() === "変換中",
    );
    wire.fire.inputCancelled("tok3");
    check("a cancelled wait returns the input to disabled", !editable());
    dispose();
  }

  // --- The placeholder names the gate's state --------------------------------

  {
    const { wire, editorEl, dispose } = harness();
    const placeholder = () => editorEl.querySelector("p")?.getAttribute("data-placeholder");
    check(
      "the placeholder names the blocker while no wait is open",
      placeholder() === "The agent is working; the input opens when it asks",
    );
    wire.fire.inputRequired("tok");
    check(
      "the placeholder invites a message once a wait is pinned",
      placeholder() === "Message the agent",
    );
    wire.fire.inputCancelled("tok");
    check(
      "the placeholder returns to the blocker when the wait dies",
      placeholder() === "The agent is working; the input opens when it asks",
    );
    dispose();
  }

  // --- The toolbar mounts between the feed and the input bar ---------------

  {
    const { view, dispose } = harness();
    check(
      "a view built without a model service mounts no toolbar",
      view.element.querySelector(".agent-toolbar") === null,
    );
    dispose();
  }

  {
    const sent = [];
    const modelService = new ModelService((id) => {
      sent.push(id);
      return true;
    });
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const view = new AgentSessionView(service, silentStatus, modelService);
    window.document.body.appendChild(view.element);
    const toolbar = view.element.querySelector(".agent-toolbar");
    check(
      "the toolbar mounts between the feed and the input bar",
      toolbar !== null &&
        view.element.querySelector(".agent-session__feed")?.nextElementSibling === toolbar &&
        toolbar.nextElementSibling === view.element.querySelector(".agent-session__bar"),
    );
    check(
      "the toolbar composes the mode chip, the model picker, and the context ring",
      toolbar?.querySelector(".mode-chip__label")?.textContent === "Agent" &&
        toolbar?.querySelector(".model-picker-trigger__label")?.textContent === "Select model" &&
        toolbar?.querySelector(".token-ring")?.getAttribute("aria-valuenow") === "0",
    );
    modelService.setModels([
      { id: "alpha", description: "first" },
      { id: "beta", description: "second" },
    ]);
    modelService.applySelected("alpha");
    check(
      "the picker shows the service's current model",
      toolbar?.querySelector(".model-picker-trigger__label")?.textContent === "alpha",
    );
    toolbar?.querySelector(".model-picker-trigger")?.click();
    const modelItems = [...document.querySelectorAll(".workshop-dropdown__item")];
    check(
      "the picker dropdown lists the catalog",
      modelItems.length === 2 &&
        modelItems[1]?.querySelector(".workshop-dropdown__label")?.textContent === "beta",
    );
    modelItems[1]?.click();
    check(
      "picking a model sends the selection through the service",
      isDeepStrictEqual(sent, ["beta"]),
    );
    let modeEvent = null;
    const onMode = (event) => {
      modeEvent = event.detail;
    };
    document.addEventListener("agent-mode-changed", onMode);
    toolbar?.querySelector(".mode-chip")?.click();
    const planItem = [...document.querySelectorAll(".workshop-dropdown__item")].find(
      (item) => item.querySelector(".workshop-dropdown__label")?.textContent === "Plan",
    );
    planItem?.click();
    document.removeEventListener("agent-mode-changed", onMode);
    check(
      "picking a mode fires agent-mode-changed and updates the chip",
      modeEvent === "plan" &&
        toolbar?.querySelector(".mode-chip__label")?.textContent === "Plan",
    );
    view.dispose();
    service.dispose();
    modelService.dispose();
    view.element.remove();
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
