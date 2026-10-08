// The agent-session view (src/parts/agent/agent-session-view.ts) in jsdom,
// driven through the real AgentSessionService over a scripted wire. The
// transcript paints the way Cursor draws it: turn wrappers holding a
// sticky human message (no "You" label) and keyed rows - replies as
// sanitized markdown with no model label, thinking as a collapsible that
// stays closed while it streams, tool calls as one-line rows with their
// results folded in, and groups of steps with a summary. Streaming deltas
// update their rows in place: settled history keeps its nodes, and an
// opened thought, group, or tool line stays as the operator left it. The
// tail status names the agent's phase (Planning next moves, a running
// tool, Reconnecting...), a finished turn's footer copies its replies and
// raises the toast, the context menu offers Copy Message / Select All /
// Search with Google, and an error opens the composer's popup instead of
// a row. Reduced motion drops the height and fade animations. The input
// pins to the pending wait, answers it byte-exact, and returns to
// disabled; a view built with a ModelService mounts the toolbar (mode
// chip, model picker, context ring) between the feed and the input bar.
// Run: node test/agent-session-view.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { mock } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { Emitter } from "@workshop/platform/event";
      export { registerService } from "@workshop/platform/service-registry";
      export { ICON_CHECK, ICON_COPY } from "@workshop/look/icons";
      export { AgentSessionService } from "./src/services/agent-session.ts";
      export { ModelService } from "./src/services/model-service.ts";
      export { TOAST_STACK } from "./src/services/toast-service.ts";
      export { AgentSessionView } from "./src/parts/agent/agent-session-view.ts";
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
// schedules with; the prompt input's editor mounts in every setup.
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

// The clipboard the copy paths write to, recorded.
const clipboard = [];
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {
    // The editor's keyboard shortcuts probe the platform.
    platform: "Win32",
    userAgent: "jsdom",
    clipboard: {
      writeText: async (text) => {
        clipboard.push(text);
      },
    },
  },
});

// jsdom has no layout: a human message overflows its clip when its text is
// longer than 60 characters, which the scripted scrollHeight reports.
Object.defineProperty(window.Element.prototype, "scrollHeight", {
  configurable: true,
  get() {
    return this.classList?.contains("ws-human-message__text") && this.textContent.length > 60 ? 100 : 20;
  },
});

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-session-view-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  lifecycle,
  Emitter,
  registerService,
  ICON_CHECK,
  ICON_COPY,
  AgentSessionService,
  AgentSessionView,
  ModelService,
  TOAST_STACK,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The shared toast stack the composition root registers, recorded.
const toasts = [];
registerService(TOAST_STACK, () => ({
  element: document.createElement("div"),
  show: (message, kind) => toasts.push({ message, kind }),
}));

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
    disconnect: new Emitter(),
  };
  return {
    onAgents: emitters.agents.event,
    onSession: emitters.session.event,
    onEvent: emitters.event.event,
    onDelta: emitters.delta.event,
    onInputRequired: emitters.inputRequired.event,
    onInputCancelled: emitters.inputCancelled.event,
    onError: emitters.error.event,
    onDisconnect: emitters.disconnect.event,
    responses: [],
    cancels: 0,
    up: true,
    launch() {
      return true;
    },
    respond(token, text) {
      if (!this.up) return false;
      this.responses.push([token, text]);
      return true;
    },
    cancelTurn() {
      this.cancels += 1;
      return true;
    },
    fire: {
      event: (kind, content, extra = {}) => {
        const { reply, ...eventFields } = extra;
        const frame = {
          type: "agent_event",
          index: 0,
          event: { kind, section: "chat", turn: 0, content, ...eventFields },
        };
        if (reply !== undefined) frame.reply = reply;
        emitters.event.fire(frame);
      },
      delta: (kind, content, reply) =>
        emitters.delta.fire({ type: "agent_delta", kind, content, reply }),
      inputRequired: (token) => emitters.inputRequired.fire(token),
      inputCancelled: (token) => emitters.inputCancelled.fire(token),
      error: (message) => emitters.error.fire(message),
      disconnect: () => emitters.disconnect.fire(),
      session: (session) => emitters.session.fire({ type: "agent_session", session, agent: "chat" }),
    },
  };
}

// Dictation's status sink: this suite never records, so nothing lands here.
const silentStatus = { showLocal() {}, setRecording() {} };

function setup() {
  const wire = makeWire();
  const service = new AgentSessionService(wire);
  const view = new AgentSessionView(service, silentStatus);
  window.document.body.appendChild(view.element);
  const rows = () => [...view.element.querySelectorAll(".ws-transcript-row")];
  const turns = () => [...view.element.querySelectorAll(".ws-turn")];
  const tail = () => view.element.querySelector(".ws-tail");
  // The chat box: content and selection are driven through the component
  // (the DOM alone sets neither on a ProseMirror editor), and the
  // pending-wait gate shows on the editor's contenteditable attribute.
  const input = view.chatBox;
  const editorEl = view.element.querySelector(".ws-prompt-input__editor");
  const editable = () => editorEl.getAttribute("contenteditable") === "true";
  // The one round action button: the mic over an empty box, the send arrow
  // once there is text, Stop while the agent generates.
  const send = view.element.querySelector(".ws-agent-session__action");
  // The operator's turn: the wait opens, the text goes, and the server
  // records it as a user message - which also leaves the agent generating.
  const ask = (text, token = `tok-${wire.responses.length}`) => {
    wire.fire.inputRequired(token);
    input.setText(text);
    send.click();
    wire.fire.event("user_message", text);
  };
  const dispose = () => {
    view.dispose();
    service.dispose();
    view.element.remove();
    for (const leftover of document.querySelectorAll(".menu-popup")) leftover.remove();
  };
  return { wire, service, view, rows, turns, tail, input, editorEl, editable, send, ask, dispose };
}

// A header's action and details text, for a collapsible-shaped element.
const actionOf = (element) => element?.querySelector(".ws-collapsible__action")?.textContent ?? null;
const detailsOf = (element) => element?.querySelector(".ws-collapsible__details")?.textContent ?? null;
const headerOf = (element) => element?.querySelector(".ws-collapsible__header") ?? null;
const isOpen = (element) => element?.dataset.open === "true";
const shimmering = (element) => element?.classList.contains("ws-shimmer-text") === true;
const settleMicrotasks = () => new Promise((resolve) => setTimeout(resolve, 0));
// Drains the promise queue without a timer, for the steps that fake setTimeout.
const flushMicrotasks = async () => {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
};

await assertNoLeaks(lifecycle, async () => {
  // --- Durable events paint turns of keyed rows ---------------------------------

  {
    const { wire, view, rows, turns, dispose } = setup();
    wire.fire.event("user_message", "hi <b>there</b>");
    wire.fire.event("agent_message", "hello back", { model: "llama-3", reply: 0 });
    wire.fire.event("agent_thought", "step one", { model: "llama-3", reply: 1 });
    wire.fire.event(
      "tool_call",
      '[{"id":"call_1","name":"read","arguments":{"path":"<script>a</script>"}}]',
      { model: "llama-3", reply: 1 },
    );
    wire.fire.event("tool_call_update", "<b>the file body</b>", { tool_call_id: "call_1" });
    wire.fire.event("agent_message", "**bold** `code` <script>alert(1)</script>", {
      model: "llama-3",
      reply: 2,
    });

    const [human, hello, group, last] = rows();
    check("one turn holds the human row, two replies, and a group; the result is no row", turns().length === 1 && rows().length === 4);
    check(
      "the human row is a card with no label",
      human?.classList.contains("ws-human-message") === true &&
        human.querySelector(".ws-human-message__text")?.textContent === "hi <b>there</b>",
    );
    check(
      "no row carries a You label or a model label",
      !view.element.textContent.includes("You") && !view.element.textContent.includes("llama-3"),
    );
    check(
      "untrusted user text lands as text, never markup",
      human?.querySelector("b") === null,
    );
    check(
      "the turn wrapper starts with its human row",
      turns()[0]?.firstElementChild === human,
    );
    check(
      "a reply is a markdown row with no bubble",
      hello?.classList.contains("ws-markdown-row") === true &&
        hello.querySelector(".ws-markdown-content")?.textContent === "hello back",
    );
    check(
      "a thought and a call that ran together are one group row",
      group?.classList.contains("ws-group-row") === true,
    );
    const groupCollapsible = group?.firstElementChild;
    check(
      "the group's header is its summary, Explored with the tool count",
      actionOf(groupCollapsible) === "Explored" && detailsOf(groupCollapsible) === "1 tool",
    );
    check("a group starts closed", !isOpen(groupCollapsible) && groupCollapsible?.querySelector(".ws-collapsible__clip")?.hasAttribute("inert") === true);
    headerOf(groupCollapsible)?.click();
    check("clicking a group's header opens it", isOpen(groupCollapsible));
    const steps = [...group.querySelectorAll(".ws-group-row__steps > .ws-collapsible")];
    check(
      "the group's steps are an inner thinking line and a tool line in a muted scope",
      steps.length === 2 &&
        steps[0]?.classList.contains("ws-thinking-line") &&
        steps[1]?.classList.contains("ws-tool-line") &&
        group.querySelector(".ws-group-row__steps")?.classList.contains("ws-muted-scope") === true,
    );
    check(
      "a settled thought reads Thought briefly",
      actionOf(steps[0]) === "Thought" && detailsOf(steps[0]) === "briefly",
    );
    check(
      "a finished tool reads Ran with its call name",
      actionOf(steps[1]) === "Ran" && detailsOf(steps[1]) === "read",
    );
    headerOf(steps[1])?.click();
    check(
      "the result folds into the tool line's block after the arguments and a blank line",
      steps[1]?.querySelector(".ws-tool-block__pre")?.textContent ===
        '{\n  "path": "<script>a</script>"\n}\n\n<b>the file body</b>',
    );
    check(
      "tool arguments and output are inert text",
      group.querySelector("script") === null && group.querySelector("b") === null,
    );
    check(
      "a reply renders its markdown formatting, not the raw source",
      last?.querySelector(".ws-markdown-content strong")?.textContent === "bold" &&
        last?.querySelector(".ws-markdown-content code")?.textContent === "code",
    );
    check("model-authored markup is sanitized before it lands", last?.querySelector("script") === null);
    dispose();
  }

  // --- Streaming: rows update in place, history keeps its nodes -------------------

  {
    const { wire, rows, tail, ask, dispose } = setup();
    ask("question");
    const humanRow = rows()[0];
    wire.fire.delta("reasoning", "let me ", 0);
    wire.fire.delta("reasoning", "think", 0);
    const thought = rows()[1];
    check(
      "reasoning deltas paint one thought row reading Thinking",
      rows().length === 2 &&
        thought?.classList.contains("ws-thought-row") &&
        actionOf(thought) === "Thinking" &&
        detailsOf(thought) === "",
    );
    check("a streaming thought stays closed", !isOpen(thought));
    check(
      "the thinking header shimmers in the thinking tone",
      shimmering(thought?.querySelector(".ws-collapsible__action")) &&
        thought.querySelector(".ws-collapsible__action").classList.contains("ws-shimmer-text--thinking"),
    );
    check("a thought row that shimmers on its own leaves no tail status", tail() === null);

    headerOf(thought)?.click();
    check(
      "the operator opens a streaming thought and reads the thinking so far",
      isOpen(thought) && thought.querySelector(".ws-thinking-body")?.textContent === "let me think",
    );
    wire.fire.delta("reasoning", "!", 0);
    check(
      "a delta updates the open thought in place",
      rows()[1] === thought &&
        isOpen(thought) &&
        thought.querySelector(".ws-thinking-body")?.textContent === "let me think!",
    );
    wire.fire.event("agent_thought", "let me think!", { model: "m", reply: 0 });
    check(
      "the settled thought is the same node, still open, no longer shimmering",
      rows()[1] === thought &&
        isOpen(thought) &&
        actionOf(thought) === "Thought" &&
        detailsOf(thought) === "briefly" &&
        !shimmering(thought.querySelector(".ws-collapsible__action")),
    );

    wire.fire.delta("text", "the ans", 0);
    const streamingReply = rows()[2];
    wire.fire.delta("text", "wer", 0);
    check(
      "text deltas paint one markdown row that updates in place",
      rows().length === 3 &&
        rows()[2] === streamingReply &&
        streamingReply?.dataset.streaming === "true" &&
        streamingReply.querySelector(".ws-markdown-content")?.textContent === "the answer",
    );
    check("the streaming reply row is the tail: no status under it", tail() === null);
    wire.fire.event("agent_message", "the answer", { model: "m", reply: 0 });
    check(
      "the durable reply settles the same node",
      rows().length === 3 &&
        rows()[2] === streamingReply &&
        streamingReply?.dataset.streaming === "false",
    );
    check(
      "settled history keeps its nodes across every delta",
      rows()[0] === humanRow && rows()[1] === thought,
    );
    dispose();
  }

  // --- Tool lines: the verb shimmers while loading; the result folds in -------------

  {
    const { wire, rows, ask, dispose } = setup();
    ask("go");
    wire.fire.event(
      "tool_call",
      '[{"id":"call_9","name":"read","tool":"fs/read","arguments":{"path":"a"}}]',
      { reply: 0 },
    );
    const line = rows()[1];
    check(
      "a lone call is a tool row, no group",
      rows().length === 2 && line?.classList.contains("ws-tool-line"),
    );
    check(
      "a running call reads Running with the call name in the verb's color",
      actionOf(line) === "Running" &&
        detailsOf(line) === "read in fs" &&
        line.querySelector(".ws-collapsible__call")?.textContent === "read",
    );
    check(
      "only the verb shimmers while the call loads",
      shimmering(line?.querySelector(".ws-collapsible__action")) &&
        line.querySelector(".ws-collapsible__action").classList.contains("ws-shimmer-text--tool") &&
        !shimmering(line.querySelector(".ws-collapsible__details")),
    );
    check("a standalone running tool row leaves no tail status", view_tail(line) === null);
    headerOf(line)?.click();
    check(
      "the operator opens the block while the call runs",
      isOpen(line) && line.querySelector(".ws-tool-block__pre")?.textContent === '{\n  "path": "a"\n}',
    );
    wire.fire.event("tool_call_update", "the body", { tool_call_id: "call_9" });
    check(
      "the landing result updates the same row: Ran, still open, no shimmer",
      rows()[1] === line &&
        rows().length === 2 &&
        actionOf(line) === "Ran" &&
        isOpen(line) &&
        !shimmering(line.querySelector(".ws-collapsible__action")),
    );
    check(
      "the block now ends with the result",
      line.querySelector(".ws-tool-block__pre")?.textContent === '{\n  "path": "a"\n}\n\nthe body',
    );
    dispose();
  }

  // A standalone tool row hides the tail by itself; this stays a helper so
  // the assertion above reads as the rule it checks.
  function view_tail(line) {
    return line.closest(".ws-turn")?.querySelector(":scope > .ws-tail") ?? null;
  }

  // --- An unparsed tool batch paints nothing ----------------------------------------

  {
    const { wire, rows, dispose } = setup();
    wire.fire.event("tool_call", "not json at all", { model: "m", reply: 0 });
    check("a batch that does not parse has no calls and no rows", rows().length === 0);
    dispose();
  }

  // --- Groups: the summary tracks the steps, open state survives ---------------------

  {
    const { wire, rows, ask, dispose } = setup();
    ask("explore");
    const callOf = (id) => `[{"id":"${id}","name":"read","tool":"fs/read","arguments":{}}]`;
    wire.fire.event("tool_call", callOf("a"), { reply: 0 });
    wire.fire.event("tool_call", callOf("b"), { reply: 1 });
    const group = rows()[1];
    const collapsible = group?.firstElementChild;
    const steps = () => [...group.querySelectorAll(".ws-group-row__steps > .ws-collapsible")];
    check(
      "two calls that ran together are a group reading Exploring while the turn runs",
      group?.classList.contains("ws-group-row") &&
        actionOf(collapsible) === "Exploring" &&
        detailsOf(collapsible) === "2 tools",
    );
    headerOf(collapsible)?.click();
    wire.fire.event("tool_call", callOf("c"), { reply: 2 });
    check(
      "a new step joins the open group in place: same node, still open, a new count",
      rows()[1] === group && isOpen(collapsible) && detailsOf(collapsible) === "3 tools" && steps().length === 3,
    );

    wire.fire.delta("reasoning", "pondering", 3);
    let thinking = steps().find((step) => step.classList.contains("ws-thinking-line"));
    check(
      "a streaming thinking step in a group is open by default",
      thinking !== undefined && isOpen(thinking) && actionOf(thinking) === "Thinking",
    );
    wire.fire.event("agent_thought", "pondering", { reply: 3 });
    check(
      "the same thinking step closes once settled",
      steps().find((step) => step.classList.contains("ws-thinking-line")) === thinking && !isOpen(thinking),
    );
    headerOf(thinking)?.click();
    wire.fire.event("tool_call", callOf("d"), { reply: 4 });
    check("a thinking step the operator opened stays open", isOpen(thinking) && isOpen(collapsible));

    wire.fire.delta("reasoning", "again", 5);
    thinking = steps().filter((step) => step.classList.contains("ws-thinking-line")).at(-1);
    headerOf(thinking)?.click();
    check("the operator closes a streaming thinking step", !isOpen(thinking));
    wire.fire.event("agent_thought", "again", { reply: 5 });
    check("a thinking step the operator closed stays closed through the settle", !isOpen(thinking));

    wire.fire.event("agent_message", "done", { reply: 6 });
    check(
      "the group reads Explored once the turn ends, in the same node",
      rows()[1] === group && actionOf(collapsible) === "Explored" && isOpen(collapsible),
    );
    dispose();
  }

  // --- A row that grows into a group keeps what the operator opened -----------------------

  {
    // A thought the operator opened, then a tool call joins it: the row becomes a
    // group under the same key, and the thinking they were reading stays open.
    const { wire, rows, ask, dispose } = setup();
    ask("think then act");
    wire.fire.delta("reasoning", "weighing options", 0);
    const thought = rows()[1];
    headerOf(thought)?.click();
    wire.fire.event("agent_thought", "weighing options", { reply: 0 });
    wire.fire.event("tool_call", '[{"id":"a","name":"read","tool":"fs/read","arguments":{}}]', { reply: 0 });
    const group = rows()[1];
    const collapsible = group?.firstElementChild;
    const thinking = group?.querySelector(".ws-thinking-line");
    check(
      "a tool call joining an opened thought makes a group in the thought's place",
      rows().length === 2 &&
        group !== thought &&
        group?.classList.contains("ws-group-row") === true &&
        group.dataset.rowKey === thought?.dataset.rowKey &&
        !thought?.isConnected,
    );
    check(
      "the group's thinking step keeps the open state the operator gave the thought",
      thinking !== null && isOpen(thinking) && thinking.querySelector(".ws-thinking-body")?.textContent === "weighing options",
    );
    check("the group opens too, so the thinking stays on screen", isOpen(collapsible));
    dispose();
  }

  {
    // The same for a thought the operator never touched: the group stays closed.
    const { wire, rows, ask, dispose } = setup();
    ask("think then act");
    wire.fire.delta("reasoning", "weighing options", 0);
    wire.fire.event("agent_thought", "weighing options", { reply: 0 });
    wire.fire.event("tool_call", '[{"id":"a","name":"read","tool":"fs/read","arguments":{}}]', { reply: 0 });
    const group = rows()[1];
    check(
      "a thought nobody opened becomes a closed group with a closed thinking step",
      group?.classList.contains("ws-group-row") === true &&
        !isOpen(group.firstElementChild) &&
        !isOpen(group.querySelector(".ws-thinking-line")),
    );
    dispose();
  }

  {
    // A lone tool row whose block the operator opened, then a second call joins it.
    const { wire, rows, ask, dispose } = setup();
    ask("two reads");
    const callOf = (id, path) => `[{"id":"${id}","name":"read","tool":"fs/read","arguments":{"path":"${path}"}}]`;
    wire.fire.event("tool_call", callOf("a", "one"), { reply: 0 });
    const line = rows()[1];
    headerOf(line)?.click();
    wire.fire.event("tool_call_update", "first body", { tool_call_id: "a" });
    wire.fire.event("tool_call", callOf("b", "two"), { reply: 1 });
    const group = rows()[1];
    const steps = [...(group?.querySelectorAll(".ws-group-row__steps > .ws-collapsible") ?? [])];
    check(
      "a second call joining an opened tool row makes a group in the row's place",
      rows().length === 2 &&
        group !== line &&
        group?.classList.contains("ws-group-row") === true &&
        group.dataset.rowKey === line?.dataset.rowKey &&
        steps.length === 2 &&
        !line?.isConnected,
    );
    check(
      "the first tool step keeps the block the operator opened, with its result",
      isOpen(steps[0]) &&
        steps[0].querySelector(".ws-tool-block__pre")?.textContent === '{\n  "path": "one"\n}\n\nfirst body',
    );
    check("the second tool step is untouched and closed", !isOpen(steps[1]));
    check("the group opens too, so the opened block stays on screen", isOpen(group?.firstElementChild));
    dispose();
  }

  {
    // A lone tool row nobody opened stays closed as a group.
    const { wire, rows, ask, dispose } = setup();
    ask("two reads");
    const callOf = (id) => `[{"id":"${id}","name":"read","tool":"fs/read","arguments":{}}]`;
    wire.fire.event("tool_call", callOf("a"), { reply: 0 });
    wire.fire.event("tool_call", callOf("b"), { reply: 1 });
    const group = rows()[1];
    check(
      "a tool row nobody opened becomes a closed group",
      group?.classList.contains("ws-group-row") === true && !isOpen(group.firstElementChild),
    );
    dispose();
  }

  // --- The tail status names the agent's phase -------------------------------------------

  {
    const { wire, rows, turns, tail, service, ask, dispose } = setup();
    ask("hi");
    const planning = tail();
    check(
      "a running turn with nothing else reads Planning next moves, last in its turn",
      planning?.querySelector(".ws-tail__action")?.textContent === "Planning next moves" &&
        planning.parentElement === turns()[0] &&
        turns()[0].lastElementChild === planning,
    );
    check(
      "the tail's verb shimmers in the status tone",
      shimmering(planning?.querySelector(".ws-tail__action")) &&
        planning.querySelector(".ws-tail__action").classList.contains("ws-shimmer-text--status"),
    );
    check("the tail has no Cancel button: stopping belongs to the composer", planning?.querySelector("button") === null);

    wire.fire.delta("reasoning", "x", 0);
    check("streaming thinking is the status: no tail", tail() === null);

    wire.fire.event("agent_thought", "x", { reply: 0 });
    wire.fire.event("tool_call", '[{"id":"a","name":"read","tool":"fs/read","arguments":{"p":1}}]', { reply: 0 });
    const group = rows()[1];
    const inGroup = tail();
    check(
      "a running tool in a group shows its verb and details in the tail inside the group",
      group?.classList.contains("ws-group-row") &&
        inGroup?.parentElement === group &&
        inGroup.querySelector(".ws-tail__action")?.textContent === "Running" &&
        inGroup.querySelector(".ws-tail__details")?.textContent === "read in fs" &&
        inGroup.querySelector(".ws-tail__call")?.textContent === "read" &&
        inGroup.querySelector(".ws-tail__action").classList.contains("ws-shimmer-text--tool"),
    );
    check("the group's tail stays visible whether or not the group is open", !isOpen(group.firstElementChild) && inGroup?.isConnected);

    wire.fire.event("tool_call_update", "ok", { tool_call_id: "a" });
    check(
      "once the call finishes the tail plans again, still inside the group",
      tail()?.parentElement === group &&
        tail().querySelector(".ws-tail__action")?.textContent === "Planning next moves",
    );

    wire.fire.delta("text", "answering", 1);
    check("streaming reply text is the status: no tail", tail() === null);
    wire.fire.event("agent_message", "answering", { reply: 1 });
    check("a finished turn has no tail", tail() === null && service.generating === false);
    dispose();
  }

  {
    const { wire, tail, service, ask, dispose } = setup();
    wire.fire.session("s1");
    ask("hi");
    wire.fire.disconnect();
    const status = tail();
    check(
      "a dropped socket reads Reconnecting... with three ASCII dots",
      service.reconnecting === true && status?.querySelector(".ws-tail__action")?.textContent === "Reconnecting...",
    );
    check("reconnecting has no Cancel action", status?.querySelector("button") === null);
    wire.fire.session("s1");
    check(
      "the acknowledged reattach returns the status to planning, still with no Cancel",
      service.reconnecting === false &&
        tail()?.querySelector(".ws-tail__action")?.textContent === "Planning next moves" &&
        tail().querySelector("button") === null,
    );
    dispose();
  }

  // --- A finished turn's footer copies its replies and raises the toast ----------------

  {
    const { wire, turns, ask, dispose } = setup();
    ask("first");
    wire.fire.event("agent_message", "reply **one**", { reply: 0 });
    const footerOf = (turn) => turn?.querySelector(":scope > .ws-turn-footer") ?? null;
    check("a finished turn shows a footer with Copy", footerOf(turns()[0]) !== null);
    ask("second");
    wire.fire.delta("text", "reply t", 1);
    check(
      "every turn but the last keeps its footer; the running last turn has none, even with reply text",
      turns().length === 2 &&
        footerOf(turns()[0]) !== null &&
        footerOf(turns()[1]) === null &&
        turns()[1].querySelector(".ws-markdown-row") !== null,
    );
    wire.fire.event("agent_message", "reply two", { reply: 1 });
    wire.fire.event("agent_message", "and more", { reply: 2 });
    const copy = footerOf(turns()[1])?.querySelector(".ws-turn-footer__copy");
    check("the last turn gets its footer once the agent stops", copy !== null && copy !== undefined);
    const copyPath = /d="([^"]+)"/.exec(ICON_COPY)?.[1];
    const earlierCopy = footerOf(turns()[0])?.querySelector(".ws-turn-footer__copy");
    check(
      "before any copy, each footer shows the codicon copy glyph",
      copyPath !== undefined &&
        copy?.querySelector("path")?.getAttribute("d") === copyPath &&
        earlierCopy?.querySelector("path")?.getAttribute("d") === copyPath &&
        copy?.dataset.copied === undefined,
    );
    // The check shows for two seconds, then the copy glyph returns: drive the timer by hand.
    mock.timers.enable({ apis: ["setTimeout"] });
    copy?.click();
    await flushMicrotasks();
    check(
      "Copy puts the turn's reply sources on the clipboard, joined by a blank line",
      clipboard.at(-1) === "reply two\n\nand more",
    );
    check(
      "the toast reads Message copied to clipboard",
      toasts.at(-1)?.message === "Message copied to clipboard",
    );
    const checkPath = /d="([^"]+)"/.exec(ICON_CHECK)?.[1];
    check(
      "the icon swaps to a check",
      checkPath !== undefined &&
        checkPath !== copyPath &&
        copy?.querySelector("path")?.getAttribute("d") === checkPath &&
        copy?.dataset.copied === "true",
    );
    mock.timers.tick(1999);
    check(
      "the check holds until two seconds have passed",
      copy?.querySelector("path")?.getAttribute("d") === checkPath && copy?.dataset.copied === "true",
    );
    mock.timers.tick(1);
    check(
      "two seconds after the copy the copy glyph returns",
      copy?.querySelector("path")?.getAttribute("d") === copyPath && copy?.dataset.copied === undefined,
    );
    footerOf(turns()[0])?.querySelector(".ws-turn-footer__copy")?.click();
    await flushMicrotasks();
    check("an earlier turn copies its own reply", clipboard.at(-1) === "reply **one**");
    // Disposed while the timers are still faked, so the footer's pending timer clears with its own clock.
    dispose();
    mock.timers.reset();
  }

  // --- The transcript context menu ----------------------------------------------------

  {
    const { wire, rows, ask, dispose } = setup();
    ask("question text");
    wire.fire.event("agent_message", "answer **md**", { reply: 0 });
    const reply = rows()[1];
    const fire = (target) =>
      target.dispatchEvent(
        new window.MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 20, clientY: 30 }),
      );
    const items = () => [...document.querySelectorAll(".menu-popup .menu-item")];
    const labels = () => items().map((item) => item.querySelector(".menu-item__label")?.textContent);
    const searches = [];
    const onClick = (event) => {
      if (event.target.tagName === "A") {
        event.preventDefault();
        searches.push(event.target.href);
      }
    };
    document.addEventListener("click", onClick, true);
    window.getSelection().removeAllRanges();

    fire(reply.querySelector(".ws-markdown-content"));
    check(
      "right-clicking a message offers Copy Message, Select All, and Search with Google",
      isDeepStrictEqual(labels(), ["Copy Message", "Select All", "Search with Google"]),
    );
    items()[0]?.click();
    await settleMicrotasks();
    check(
      "Copy Message copies the message's markdown source and closes the menu",
      clipboard.at(-1) === "answer **md**" && items().length === 0,
    );

    const range = document.createRange();
    range.selectNodeContents(reply.querySelector("p"));
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
    fire(reply.querySelector(".ws-markdown-content"));
    check("over a selection the first item reads Copy", labels()[0] === "Copy");
    items()[0]?.click();
    await settleMicrotasks();
    check("Copy copies the selected text", clipboard.at(-1) === "answer md");

    // The menu takes focus, which jsdom answers by clearing the selection.
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
    fire(reply.querySelector(".ws-markdown-content"));
    items()[2]?.click();
    check(
      "Search with Google opens a search for the selection",
      searches.at(-1) === "https://www.google.com/search?q=answer%20md",
    );

    window.getSelection().removeAllRanges();
    fire(reply.querySelector(".ws-markdown-content"));
    items()[1]?.click();
    const everything = window.getSelection().toString();
    check(
      "Select All selects the whole transcript",
      everything.includes("question text") && everything.includes("answer md"),
    );
    window.getSelection().removeAllRanges();
    document.removeEventListener("click", onClick, true);
    dispose();
  }

  // A right-click outside every row has nothing to copy: no Copy item, and the clipboard is left alone.

  {
    const { wire, turns, tail, ask, dispose } = setup();
    ask("question text");
    window.getSelection().removeAllRanges();
    const fire = (target) =>
      target.dispatchEvent(
        new window.MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 20, clientY: 30 }),
      );
    const items = () => [...document.querySelectorAll(".menu-popup .menu-item")];
    const labels = () => items().map((item) => item.querySelector(".menu-item__label")?.textContent);
    clipboard.length = 0;

    fire(tail());
    check(
      "right-clicking the tail offers no Copy Message",
      isDeepStrictEqual(labels(), ["Select All", "Search with Google"]),
    );
    items()[0]?.click();
    await settleMicrotasks();
    check("choosing from the tail's menu leaves the clipboard alone", clipboard.length === 0);
    // Select All left the transcript selected; the next right-click starts clean.
    window.getSelection().removeAllRanges();

    wire.fire.event("agent_message", "an answer", { reply: 0 });
    const footer = turns()[0]?.querySelector(":scope > .ws-turn-footer");
    check("the finished turn has its footer", footer !== null && footer !== undefined);
    fire(footer);
    check(
      "right-clicking a turn footer offers no Copy Message",
      isDeepStrictEqual(labels(), ["Select All", "Search with Google"]),
    );
    items()[0]?.click();
    await settleMicrotasks();
    window.getSelection().removeAllRanges();

    fire(turns()[0]?.querySelector(".ws-markdown-content"));
    check(
      "a row in the same turn still offers Copy Message",
      isDeepStrictEqual(labels(), ["Copy Message", "Select All", "Search with Google"]),
    );
    items()[0]?.click();
    await settleMicrotasks();
    check("and copies its text", isDeepStrictEqual(clipboard, ["an answer"]));
    window.getSelection().removeAllRanges();
    dispose();
  }

  // --- Errors open the composer's popup, not rows ----------------------------------------

  {
    const { wire, view, rows, input, send, dispose } = setup();
    const popup = () => view.element.querySelector(".ws-chat-error");
    const retry = () => popup()?.querySelector(".ws-chat-error__retry");
    wire.fire.error("the model call failed");
    check(
      "an error paints no row and no Error: text in the feed",
      rows().length === 0 && !view.transcript.element.textContent.includes("Error"),
    );
    check(
      "the error opens the popup titled Connection Error with its message",
      popup() !== null &&
        popup().getAttribute("role") === "alert" &&
        popup().querySelector(".ws-chat-error__title")?.textContent === "Connection Error" &&
        popup().querySelector(".ws-chat-error__message")?.textContent === "the model call failed",
    );
    check("with no message sent there is nothing to try again", retry()?.hidden === true);

    wire.fire.event("user_message", "ask me");
    wire.fire.error("the round failed");
    check(
      "after a send, Try again is offered but disabled until a wait is pinned",
      retry()?.hidden === false && retry().disabled === true,
    );
    wire.fire.inputRequired("t9");
    check("a pinned wait enables Try again", retry()?.disabled === false);
    retry().click();
    check(
      "Try again re-sends the last user message through respond",
      isDeepStrictEqual(wire.responses.at(-1), ["t9", "ask me"]) && popup() === null,
    );

    wire.up = false;
    wire.fire.inputRequired("t10");
    input.setText("hello");
    send.click();
    check(
      "a send on a downed socket opens the popup titled Connection failed",
      popup()?.querySelector(".ws-chat-error__title")?.textContent === "Connection failed" &&
        popup().querySelector(".ws-chat-error__message")?.textContent ===
          "The connection was interrupted. Please check your network connection and try again." &&
        input.getText() === "hello",
    );
    wire.fire.session("fresh");
    check("a new session clears the popup", popup() === null);
    dispose();
  }

  // --- The human message: clip, expand, sticky ---------------------------------------------

  {
    const { wire, view, ask, dispose } = setup();
    ask("short one");
    ask("x".repeat(80));
    window.getSelection().removeAllRanges();
    const [short, long] = [...view.element.querySelectorAll(".ws-human-message")];
    const textOf = (card) => card.querySelector(".ws-human-message__text");
    check(
      "a message within the clip doesn't overflow and isn't a button",
      short?.dataset.overflowing === "false" &&
        textOf(short).dataset.clipped === "false" &&
        textOf(short).getAttribute("role") === null,
    );
    check(
      "a long message is clipped and reads as clickable",
      long?.dataset.overflowing === "true" &&
        textOf(long).dataset.clipped === "true" &&
        textOf(long).getAttribute("role") === "button" &&
        textOf(long).getAttribute("aria-expanded") === "false",
    );
    textOf(long).click();
    check(
      "clicking a long message expands it",
      long.dataset.expanded === "true" &&
        textOf(long).dataset.clipped === "false" &&
        textOf(long).getAttribute("aria-expanded") === "true",
    );
    textOf(long).click();
    check("clicking again clips it", textOf(long).dataset.clipped === "true");
    textOf(short).click();
    check("a click on a short message does nothing", short.dataset.expanded === "false");
    textOf(long).click();
    wire.fire.event("agent_message", "reply", { reply: 0 });
    check("an expanded message stays expanded across later updates", long.dataset.expanded === "true");

    const feed = view.transcript.element;
    let scrollTop = 40;
    Object.defineProperty(feed, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value) => {
        scrollTop = value;
      },
    });
    Object.defineProperty(feed, "clientHeight", { configurable: true, get: () => 300 });
    const rect = (top, height) => ({ top, height, left: 0, width: 400, bottom: top + height, right: 400 });
    feed.getBoundingClientRect = () => rect(100, 300);
    short.getBoundingClientRect = () => rect(100, 50);
    feed.dispatchEvent(new window.Event("scroll"));
    check(
      "a message at the feed's top edge is stuck, which shows the fade",
      short.dataset.stuck === "true" && short.dataset.tall === "false",
    );
    short.getBoundingClientRect = () => rect(160, 50);
    feed.dispatchEvent(new window.Event("scroll"));
    check("a message below the top edge is not stuck", short.dataset.stuck === "false");
    short.getBoundingClientRect = () => rect(100, 400);
    feed.dispatchEvent(new window.Event("scroll"));
    check(
      "a message taller than the feed doesn't stick",
      short.dataset.tall === "true" && short.dataset.stuck === "false",
    );
    dispose();
  }

  // --- Motion: new text fades in, and reduced motion drops it ---------------------------------

  {
    const { wire, rows, ask, dispose } = setup();
    ask("go");
    wire.fire.delta("text", "alpha ", 0);
    wire.fire.delta("text", "beta", 0);
    const fades = () => [...(rows()[1]?.querySelectorAll(".ws-fade-in") ?? [])];
    check(
      "streaming text fades in word by word, and a word already fading carries on with a negative delay",
      fades().length === 2 &&
        fades()[0].textContent === "alpha" &&
        Number.parseFloat(fades()[0].style.animationDelay) < 0 &&
        Number.parseFloat(fades()[1].style.animationDelay) <= 0,
    );
    wire.fire.event("agent_message", "alpha beta", { reply: 0 });
    dispose();
  }

  {
    window.matchMedia = (query) => ({ matches: true, media: query });
    const { wire, rows, ask, dispose } = setup();
    ask("go");
    wire.fire.delta("reasoning", "thinking", 0);
    wire.fire.delta("text", "alpha beta", 1);
    check(
      "under reduced motion streamed text is not wrapped to fade",
      rows()[2]?.querySelector(".ws-markdown-content")?.textContent === "alpha beta" &&
        rows()[2].querySelectorAll(".ws-fade-in").length === 0,
    );
    dispose();
    delete window.matchMedia;
  }

  // The stylesheets carry the rest: the height and fade animations are
  // dropped under reduced motion, and the sticky card follows the spec.
  {
    const css = async (name) => (await readFile(path.join(testDir, "..", "src", "parts", "agent", name), "utf8")).replace(/\/\*[\s\S]*?\*\//g, "");
    const reduced = (text) => text.split("@media (prefers-reduced-motion: reduce)").slice(1).join("\n");
    const collapsible = await css("transcript/collapsible.css");
    const transcript = await css("transcript/transcript.css");
    const markdown = await css("markdown-render.css");
    const human = await css("transcript/human-message.css");
    check(
      "the collapsible's height and chevron transitions are dropped under reduced motion",
      /ws-collapsible__clip[\s\S]*ws-collapsible__chevron[^{]*\{[^}]*transition:\s*none/.test(reduced(collapsible)),
    );
    check(
      "the collapsible opens its height over 150ms on an ease-out cubic",
      /\.ws-collapsible__clip\s*\{[^}]*grid-template-rows:\s*0fr[^}]*transition:[^;]*var\(--duration-normal\) var\(--ease-out-cubic\)/.test(collapsible),
    );
    check(
      "a new reply row's fade and rise are dropped under reduced motion",
      /animation:\s*none/.test(reduced(transcript)),
    );
    check("the streaming fade is dropped under reduced motion", /\.ws-fade-in\s*\{[^}]*animation:\s*none/.test(reduced(markdown)));
    check(
      "the human message sticks at the top with z-index 101, except when taller than the feed",
      /\.ws-human-message\s*\{[^}]*position:\s*sticky[^}]*inset-block-start:\s*0[^}]*z-index:\s*101/.test(human) &&
        /\.ws-human-message\[data-tall="true"\]\s*\{[^}]*position:\s*static/.test(human),
    );
  }

  // --- Every content string is inert ----------------------------------------------------

  {
    const { wire, view, rows, dispose } = setup();
    wire.fire.event("user_message", "<img src=x onerror=alert(1)>");
    wire.fire.delta("reasoning", "**shown**\n\n<script>alert(1)</script>", 0);
    const thought = rows()[1];
    headerOf(thought)?.click();
    check(
      "user text is text: no element comes out of it",
      view.element.querySelector(".ws-human-message img") === null &&
        rows()[0].querySelector(".ws-human-message__text").textContent === "<img src=x onerror=alert(1)>",
    );
    check(
      "thinking markdown is sanitized when opened",
      thought.querySelector("script") === null &&
        thought.querySelector(".ws-thinking-body strong")?.textContent === "shown",
    );
    check("no element in the transcript carries an event handler", view.element.querySelector("[onerror], [onclick]") === null);
    dispose();
  }

  // --- The input pins to the pending wait ------------------------------------------------

  {
    const { wire, input, editorEl, editable, send, dispose } = setup();
    check(
      "the input starts disabled with no wait open, the action idle",
      !editable() && send.getAttribute("data-action") === "idle",
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
      "the spent wait returns the input to disabled, and the button to Stop while the agent generates",
      !editable() &&
        send.getAttribute("data-action") === "stop" &&
        send.getAttribute("data-state") === "stop",
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

  // --- A model selection gates every submission path ------------------------------------

  {
    const status = {
      local: [],
      showLocal(label, severity) {
        this.local.push({ label, severity });
      },
      setRecording() {},
    };
    const modelService = new ModelService(() => true);
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const view = new AgentSessionView(service, status, modelService);
    window.document.body.appendChild(view.element);
    const input = view.chatBox;
    const editorEl = view.element.querySelector(".ws-prompt-input__editor");
    const send = view.element.querySelector(".ws-agent-session__action");
    check(
      "with no wait the action is idle whatever the model state, and the empty box's mic stays pressable",
      send.getAttribute("data-action") === "idle" &&
        send.getAttribute("data-state") === "mic" &&
        send.disabled === false,
    );
    wire.fire.inputRequired("model-gated");
    input.setText("keep this draft");
    check(
      "the send control exposes the absent-selection gate",
      send.getAttribute("aria-disabled") === "true",
    );
    check(
      "the absent selection maps to the send-blocked action, still clickable",
      send.getAttribute("data-action") === "send-blocked" && send.disabled === false,
    );

    send.click();
    check(
      "click submission without a model is rejected and keeps the draft",
      wire.responses.length === 0 && input.getText() === "keep this draft",
    );
    check(
      "click submission without a model shows the exact local selection status",
      isDeepStrictEqual(status.local.at(-1), {
        label: "Select a model before sending.",
        severity: "info",
      }),
    );

    editorEl.dispatchEvent(
      new window.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    );
    check(
      "keyboard submission without a model is rejected and keeps the draft",
      wire.responses.length === 0 && input.getText() === "keep this draft",
    );
    check(
      "keyboard submission without a model shows the exact local selection status",
      status.local.length === 2 &&
        status.local[1]?.label === "Select a model before sending." &&
        status.local[1]?.severity === "info",
    );

    modelService.applySelected("alpha");
    check(
      "selection arrival immediately lifts the send control gate",
      send.getAttribute("aria-disabled") === "false" && send.getAttribute("data-action") === "send",
    );
    send.click();
    check(
      "a later model selection makes the pending draft immediately submittable",
      isDeepStrictEqual(wire.responses, [["model-gated", "keep this draft"]]) &&
        input.getText() === "",
    );

    view.dispose();
    service.dispose();
    modelService.dispose();
    view.element.remove();
  }

  // --- The placeholder matches Cursor's agent input ---------------------------------------

  {
    const { wire, editorEl, dispose } = setup();
    const placeholder = () => editorEl.querySelector("p")?.getAttribute("data-placeholder");
    check(
      "the input shows Cursor's agent placeholder",
      placeholder() === "Plan, Build, / for skills, @ for context",
    );
    wire.fire.inputRequired("tok");
    check(
      "the placeholder stays stable when the input opens",
      placeholder() === "Plan, Build, / for skills, @ for context",
    );
    wire.fire.inputCancelled("tok");
    check(
      "the placeholder stays stable when the input closes",
      placeholder() === "Plan, Build, / for skills, @ for context",
    );
    dispose();
  }

  // --- The toolbar mounts between the feed and the input bar -----------------------------

  {
    const { view, dispose } = setup();
    check(
      "a view built without a model service mounts no toolbar",
      view.element.querySelector(".ws-agent-toolbar") === null,
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
    const toolbar = view.element.querySelector(".ws-agent-toolbar");
    const bar = view.element.querySelector(".ws-agent-session__bar");
    check(
      "the toolbar mounts inside the input card after the prompt",
      toolbar !== null &&
        bar !== null &&
        toolbar.parentElement === bar &&
        toolbar.previousElementSibling?.classList.contains("ws-prompt-input") === true,
    );
    check(
      "the box's one round action button trails the toolbar's own controls, after the ring",
      toolbar?.lastElementChild?.classList.contains("ws-agent-session__action") === true &&
        toolbar?.lastElementChild?.previousElementSibling?.classList.contains("ws-token-ring") === true &&
        bar?.querySelector(":scope > .ws-agent-session__action") === null,
    );
    check(
      "the toolbar composes the mode chip, the model picker, and the context ring, hidden until there is usage data",
      toolbar?.querySelector(".ws-mode-chip__label")?.textContent === "Agent" &&
        toolbar?.querySelector(".ws-model-picker-trigger__label")?.textContent === "Select model" &&
        toolbar?.querySelector(".ws-token-ring")?.hasAttribute("hidden") === true,
    );
    modelService.setModels([
      { id: "alpha", description: "first" },
      { id: "beta", description: "second" },
    ]);
    modelService.applySelected("alpha");
    check(
      "the picker shows the service's current model",
      toolbar?.querySelector(".ws-model-picker-trigger__label")?.textContent === "alpha",
    );
    toolbar?.querySelector(".ws-model-picker-trigger")?.click();
    const modelItems = [...document.querySelectorAll(".menu-item")];
    check(
      "the picker dropdown lists the catalog",
      modelItems.length === 2 &&
        modelItems[1]?.querySelector(".menu-item__label")?.textContent === "beta",
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
    toolbar?.querySelector(".ws-mode-chip")?.click();
    const planItem = [...document.querySelectorAll(".menu-item")].find(
      (item) => item.querySelector(".menu-item__label")?.textContent === "Plan",
    );
    planItem?.click();
    document.removeEventListener("agent-mode-changed", onMode);
    check(
      "picking a mode fires agent-mode-changed and updates the chip",
      modeEvent === "plan" &&
        toolbar?.querySelector(".ws-mode-chip__label")?.textContent === "Plan",
    );
    view.dispose();
    service.dispose();
    modelService.dispose();
    view.element.remove();
  }

  // --- Stop: the round button cancels the running turn --------------------------------------

  {
    const { wire, service, view, send, ask, dispose } = setup();
    wire.fire.session("s-stop");
    check(
      "before any turn there is nothing to stop",
      view.cancelTurn() === false && wire.cancels === 0,
    );
    ask("go");
    check(
      "the turn the operator started leaves the button as Stop",
      service.generating === true && send.getAttribute("data-state") === "stop" && send.disabled === false,
    );
    send.click();
    check("a click on Stop cancels the turn through the wire", wire.cancels === 1);
    check(
      "the service settles its own view of the cancelled turn: generating is off",
      service.generating === false && send.getAttribute("data-state") !== "stop",
    );
    check(
      "with generating off and no wait the action is idle again",
      send.getAttribute("data-action") === "idle",
    );
    check("a turn that is not running cannot be stopped again", view.cancelTurn() === false && wire.cancels === 1);
    dispose();
  }

  {
    // The agent finishing its own turn (a reply, then a new wait) takes Stop away too.
    const { wire, service, send, ask, dispose } = setup();
    wire.fire.session("s-done");
    ask("go");
    check("generating shows Stop", send.getAttribute("data-state") === "stop");
    wire.fire.event("agent_message", "done", { reply: 0 });
    wire.fire.inputRequired("tok-next");
    check(
      "the reply and the next wait end the turn: Stop gives way to the mic over the empty box",
      service.generating === false && send.getAttribute("data-state") === "mic" && send.getAttribute("data-action") === "send",
    );
    dispose();
  }

  // --- The mode follows the toolbar's chip onto the action button ------------------------------

  {
    const modelService = new ModelService(() => true);
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const view = new AgentSessionView(service, silentStatus, modelService);
    window.document.body.appendChild(view.element);
    const button = view.element.querySelector(".ws-agent-session__action");
    check("the action button starts in Agent mode", button.getAttribute("data-mode") === "agent");
    view.element.querySelector(".ws-mode-chip")?.click();
    [...document.querySelectorAll(".menu-item")]
      .find((item) => item.querySelector(".menu-item__label")?.textContent === "Ask")
      ?.click();
    check("picking Ask in the chip colors the action button for Ask", button.getAttribute("data-mode") === "ask");
    view.dispose();
    service.dispose();
    modelService.dispose();
    view.element.remove();
  }

  // --- The commands' handle: focus, emptiness, menus, Shift+Tab ------------------------------

  {
    const modelService = new ModelService(() => true);
    modelService.setModels([{ id: "alpha" }]);
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const view = new AgentSessionView(service, silentStatus, modelService);
    window.document.body.appendChild(view.element);
    check("a fresh view is empty: a chat to reuse", view.isEmpty() === true);
    view.chatBox.setText("a draft");
    check("a draft makes the chat non-empty", view.isEmpty() === false);
    view.chatBox.clear();
    wire.fire.session("s-handle");
    wire.fire.event("user_message", "hello");
    check("a turn makes the chat non-empty", view.isEmpty() === false);

    check("focus starts outside the view", view.hasFocus() === false);
    // jsdom does not focus a contenteditable, so the composer's focus request is
    // observed on the box, and hasFocus on a control jsdom can focus.
    let focusRequests = 0;
    const realFocus = view.chatBox.focus.bind(view.chatBox);
    view.chatBox.focus = () => {
      focusRequests += 1;
      realFocus();
    };
    view.focusInput();
    check("focusInput asks the composer for focus", focusRequests === 1);
    const probe = document.createElement("button");
    view.element.appendChild(probe);
    probe.focus();
    check("hasFocus sees focus anywhere inside the view", view.hasFocus() === true);
    probe.remove();
    check("hasFocus is false again once focus leaves the view", view.hasFocus() === false);

    // Escape is how a person dismisses an open menu; it also frees the chip's own state.
    const dismissMenus = () =>
      document.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    view.openModeMenu();
    check(
      "openModeMenu opens the mode menu",
      document.querySelector(".menu-popup.ws-mode-menu") !== null,
    );
    view.openModeMenu();
    check(
      "pressing again cycles the mode",
      view.element.querySelector(".ws-mode-chip__label")?.textContent === "Plan",
    );
    dismissMenus();
    view.openModelMenu();
    check("openModelMenu opens the model menu", document.querySelector(".menu-popup.ws-model-menu") !== null);
    dismissMenus();

    // Shift+Tab in the composer opens the mode menu; it is not a registered
    // chord, so the dispatcher never claims it from the other controls.
    const editorElement = view.element.querySelector(".ws-prompt-input__editor");
    const press = (init) => {
      const event = new window.KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true, ...init });
      editorElement.dispatchEvent(event);
      return event;
    };
    const plain = press({});
    check("a plain Tab in the composer is left alone", plain.defaultPrevented === false && document.querySelector(".menu-popup") === null);
    const shifted = press({ shiftKey: true });
    check(
      "Shift+Tab in the composer opens the mode menu and is consumed",
      shifted.defaultPrevented === true && document.querySelector(".menu-popup.ws-mode-menu") !== null,
    );
    dismissMenus();
    const outside = new window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
    view.element.querySelector(".ws-chat-error, .ws-agent-session__bar")?.dispatchEvent(outside);
    check(
      "Shift+Tab outside the editor (on the bar) opens nothing",
      outside.defaultPrevented === false && document.querySelector(".menu-popup") === null,
    );
    view.dispose();
    service.dispose();
    modelService.dispose();
    view.element.remove();
  }

  {
    // Without a model service there is no toolbar, so the menu commands have nothing to open.
    const { view, dispose } = setup();
    view.openModeMenu();
    view.openModelMenu();
    check("a view without a toolbar opens no menu", document.querySelector(".menu-popup") === null);
    dispose();
  }

  // --- A new session clears the feed -----------------------------------------------------------

  {
    const { wire, rows, dispose } = setup();
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
