// The agent-session view model (src/services/agent-session.ts) against a
// scripted wire, no DOM: durable events fold into transcript items in
// log order; ephemeral deltas coalesce into pending items by their
// superseding reply id, per channel, and the durable event replaces
// them; late deltas after their round settled are dropped; the input pin
// follows input_required / input_cancelled / respond; a session
// acknowledgment resets the pin and a new session id resets the
// transcript; errors fold as items. The turn's liveness rides along: an
// injected clock stamps each thought's start and end, `generating` and
// `reconnecting` follow their rules, `cancelTurn` settles the service's
// own view, tool rows carry their bound tool id, and errors carry their
// popup title. Run: node test/agent-session-service.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { Emitter } from "@workshop/platform/event";
      export { AgentSessionService } from "./src/services/agent-session.ts";
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
});

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-session-service-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, Emitter, AgentSessionService } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The scripted wire: the AgentSocket surface the service consumes, with
// test-side fire methods and send recorders.
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
    launched: [],
    responses: [],
    cancels: 0,
    launchResult: true,
    respondResult: true,
    cancelResult: true,
    launch(agent) {
      this.launched.push(agent);
      return this.launchResult;
    },
    respond(token, text) {
      this.responses.push([token, text]);
      return this.respondResult;
    },
    cancelTurn() {
      this.cancels++;
      return this.cancelResult;
    },
    fire: {
      disconnect: () => emitters.disconnect.fire(undefined),
      agents: (list) => emitters.agents.fire(list),
      session: (session, agent = "chat") =>
        emitters.session.fire({ type: "agent_session", session, agent }),
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
    },
    disposeEmitters: () => {
      for (const emitter of Object.values(emitters)) emitter.dispose();
    },
  };
}

await assertNoLeaks(lifecycle, () => {
  // --- Durable events fold into transcript items in log order --------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    let transcriptFires = 0;
    service.onDidChangeTranscript(() => transcriptFires++);
    wire.fire.event("user_message", "hi there");
    wire.fire.event("agent_thought", "let me see", { model: "llama-3", reply: 0 });
    wire.fire.event("agent_message", "hello", { model: "llama-3", reply: 0 });
    wire.fire.event("tool_call", '[{"id":"call_1","name":"read","arguments":{"path":"a"}}]', {
      model: "llama-3",
      reply: 1,
    });
    wire.fire.event("tool_call_update", "file body", { tool_call_id: "call_1" });
    const kinds = service.items.map((item) => item.kind);
    check(
      "durable events fold in log order",
      isDeepStrictEqual(kinds, ["user", "reasoning", "reply", "tool-call", "tool-result"]),
    );
    check(
      "the user item holds the byte-exact text",
      service.items[0].text === "hi there",
    );
    check(
      "reply and reasoning items record their model label",
      service.items[1].model === "llama-3" && service.items[2].model === "llama-3",
    );
    check(
      "a settled durable item is not pending",
      service.items[1].pending === false && service.items[2].pending === false,
    );
    check(
      "the tool-call batch parses into one row per call",
      isDeepStrictEqual(service.items[3].calls, [
        { id: "call_1", name: "read", args: '{"path":"a"}', tool: null },
      ]),
    );
    check(
      "the tool result keeps its call id and content",
      service.items[4].toolCallId === "call_1" && service.items[4].text === "file body",
    );
    check("every fold fired the transcript change", transcriptFires === 5);
    service.dispose();
  }

  // --- Deltas coalesce by reply id and the durable event replaces them -----

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.delta("text", "Hel", 0);
    wire.fire.delta("text", "lo", 0);
    check(
      "text deltas coalesce into one pending reply",
      service.items.length === 1 &&
        service.items[0].kind === "reply" &&
        service.items[0].pending === true &&
        service.items[0].text === "Hello",
    );
    wire.fire.delta("reasoning", "hmm", 0);
    wire.fire.delta("reasoning", " ok", 0);
    check(
      "reasoning deltas coalesce into their own pending item",
      service.items.length === 2 &&
        service.items[1].kind === "reasoning" &&
        service.items[1].text === "hmm ok",
    );
    wire.fire.event("agent_thought", "hmm ok settled", { model: "m", reply: 0 });
    check(
      "the thought event replaces only the reasoning channel",
      service.items.length === 2 &&
        service.items[0].kind === "reply" &&
        service.items[0].pending === true &&
        service.items[1].kind === "reasoning" &&
        service.items[1].pending === false &&
        service.items[1].text === "hmm ok settled",
    );
    wire.fire.event("agent_message", "Hello there", { model: "m", reply: 0 });
    check(
      "the reply event replaces the coalesced text deltas",
      service.items.length === 2 &&
        service.items[1].kind === "reply" &&
        service.items[1].pending === false &&
        service.items[1].text === "Hello there",
    );
    wire.fire.delta("text", "late", 0);
    check(
      "a late delta after its round settled is dropped",
      service.items.length === 2,
    );
    wire.fire.delta("text", "next", 1);
    check(
      "the next round's deltas open a fresh pending reply",
      service.items.length === 3 && service.items[2].pending === true,
    );
    service.dispose();
  }

  // --- A tool-call batch settles its round's pending deltas ----------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.delta("reasoning", "planning", 0);
    wire.fire.event("tool_call", '[{"id":"c1","name":"search","arguments":{}}]', {
      model: "m",
      reply: 0,
    });
    check(
      "a tool-call batch supersedes its round's pending deltas",
      service.items.length === 1 && service.items[0].kind === "tool-call",
    );
    check(
      "the batch keeps its model label",
      service.items[0].model === "m",
    );
    service.dispose();
  }

  // --- Malformed batch content degrades to the raw text --------------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.event("tool_call", "not json", { model: "m" });
    check(
      "an unparsable tool-call batch keeps the raw text with no rows",
      service.items[0].calls.length === 0 && service.items[0].text === "not json",
    );
    service.dispose();
  }

  // --- Unknown event kinds are tolerated and render nothing ----------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    let fires = 0;
    service.onDidChangeTranscript(() => fires++);
    wire.fire.event("plan", "a future kind");
    check(
      "an unknown event kind folds nothing and fires nothing",
      service.items.length === 0 && fires === 0,
    );
    service.dispose();
  }

  // --- The input pin: required, respond, cancelled --------------------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const pins = [];
    service.onDidChangePendingInput((token) => pins.push(token));
    check("no wait is pinned at construction", service.pendingInputToken === null);
    check("respond without a pin sends nothing", service.respond("hi") === false);
    check("nothing went out without a pin", wire.responses.length === 0);
    wire.fire.inputRequired("tok1");
    check("input_required pins its token", service.pendingInputToken === "tok1");
    wire.fire.inputCancelled("other");
    check("a foreign token's cancellation leaves the pin", service.pendingInputToken === "tok1");
    check("respond sends the pinned token with the text byte-exact", service.respond("hi  ") === true);
    check(
      "the response sent token and text",
      isDeepStrictEqual(wire.responses, [["tok1", "hi  "]]),
    );
    check("a spent token unpins", service.pendingInputToken === null);
    wire.fire.inputRequired("tok2");
    wire.fire.inputCancelled("tok2");
    check("input_cancelled unpins its own token", service.pendingInputToken === null);
    check(
      "the pin change event fired for every transition",
      isDeepStrictEqual(pins, ["tok1", null, "tok2", null]),
    );
    service.dispose();
  }

  // --- A failed respond keeps the pin and folds a local error ---------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.inputRequired("tok1");
    wire.respondResult = false;
    check("a failed send reports false", service.respond("hi") === false);
    check("the pin survives a failed send", service.pendingInputToken === "tok1");
    check(
      "the failure folds as an error item",
      service.items.length === 1 && service.items[0].kind === "error",
    );
    service.dispose();
  }

  // --- Session acknowledgments: pin reset, transcript reset on a new id ----

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const sessions = [];
    service.onDidChangeSession((frame) => sessions.push(frame.session));
    // A refused launch folds a pre-session error; the session that then
    // starts leaves it out of its feed.
    wire.fire.error("unknown agent: bad");
    wire.fire.inputRequired("tok1");
    wire.fire.session("s1");
    check("an acknowledgment resets the pin for the resend set", service.pendingInputToken === null);
    check(
      "the first acknowledgment starts the session's transcript clean",
      service.items.length === 0 && service.session?.session === "s1",
    );
    wire.fire.event("user_message", "one");
    wire.fire.inputRequired("tok2");
    wire.fire.session("s1");
    check(
      "a same-session reattach keeps the transcript and resets the pin",
      service.items.length === 1 && service.pendingInputToken === null,
    );
    wire.fire.session("s2");
    check("a new session id resets the transcript", service.items.length === 0);
    check("every acknowledgment fired", isDeepStrictEqual(sessions, ["s1", "s1", "s2"]));
    service.dispose();
  }

  // --- Errors fold as items and re-fire; agents list snapshots --------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const heard = [];
    service.onError((message) => heard.push(message));
    wire.fire.error("unknown agent");
    check(
      "an error frame folds as an error item and re-fires",
      service.items[0]?.kind === "error" &&
        service.items[0].message === "unknown agent" &&
        isDeepStrictEqual(heard, ["unknown agent"]),
    );
    wire.fire.agents(["chat", "research"]);
    check(
      "the agents snapshot updates",
      isDeepStrictEqual([...service.agents], ["chat", "research"]),
    );
    check("launch forwards to the wire", service.launch("chat") === true);
    check("the launch named its agent", isDeepStrictEqual(wire.launched, ["chat"]));
    service.dispose();
  }

  // --- Reasoning timing: stamps come from the injected clock -----------------

  {
    const wire = makeWire();
    const clock = { now: 100 };
    const service = new AgentSessionService(wire, () => clock.now);
    wire.fire.delta("reasoning", "hmm", 0);
    check(
      "the first reasoning delta stamps startedAt and leaves endedAt open",
      service.items[0].startedAt === 100 && service.items[0].endedAt === null,
    );
    clock.now = 150;
    wire.fire.delta("reasoning", " ok", 0);
    check(
      "a later reasoning delta keeps the round's startedAt",
      service.items[0].text === "hmm ok" &&
        service.items[0].startedAt === 100 &&
        service.items[0].endedAt === null,
    );
    clock.now = 250;
    wire.fire.delta("text", "other round", 1);
    check(
      "a text delta of another reply leaves the thinking open",
      service.items[0].endedAt === null,
    );
    clock.now = 400;
    wire.fire.delta("text", "Hel", 0);
    check(
      "the same reply's first text delta ends the thinking",
      service.items[0].endedAt === 400 && service.items[0].startedAt === 100,
    );
    clock.now = 600;
    wire.fire.delta("text", "lo", 0);
    check("a later text delta does not move endedAt", service.items[0].endedAt === 400);
    clock.now = 700;
    wire.fire.event("agent_thought", "hmm ok settled", { model: "m", reply: 0 });
    const durable = service.items.find((item) => item.kind === "reasoning" && !item.pending);
    check(
      "the durable thought copies both stamps from the pending item",
      durable !== undefined && durable.startedAt === 100 && durable.endedAt === 400,
    );
    wire.fire.event("agent_thought", "replayed", { model: "m", reply: 7 });
    const replayed = service.items[service.items.length - 1];
    check(
      "a durable thought with no pending item has no stamps",
      replayed.startedAt === null && replayed.endedAt === null,
    );
    service.dispose();
  }

  {
    const wire = makeWire();
    const clock = { now: 1000 };
    const service = new AgentSessionService(wire, () => clock.now);
    wire.fire.delta("reasoning", "planning", 0);
    clock.now = 1300;
    wire.fire.event("agent_thought", "planning", { model: "m", reply: 0 });
    check(
      "a round that never streamed text ends its thinking at the durable thought",
      service.items[0].startedAt === 1000 && service.items[0].endedAt === 1300,
    );

    clock.now = 2000;
    wire.fire.delta("reasoning", "thinking about an error", 1);
    clock.now = 2500;
    wire.fire.error("boom");
    const afterError = service.items.find((item) => item.kind === "reasoning" && item.reply === 1);
    check("an error ends the open thinking", afterError.endedAt === 2500);

    clock.now = 3000;
    wire.fire.delta("reasoning", "waiting on input", 2);
    clock.now = 3400;
    wire.fire.inputRequired("tok1");
    const afterInput = service.items.find((item) => item.kind === "reasoning" && item.reply === 2);
    check("input_required ends the open thinking", afterInput.endedAt === 3400);
    clock.now = 9000;
    wire.fire.error("again");
    check(
      "an error does not move an already-ended thinking",
      afterInput === service.items.find((item) => item.kind === "reasoning" && item.reply === 2) &&
        service.items.find((item) => item.kind === "reasoning" && item.reply === 1).endedAt === 2500,
    );
    service.dispose();
  }

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.delta("reasoning", "default clock", 0);
    check(
      "without an injected clock the stamp is a finite performance.now() value",
      Number.isFinite(service.items[0].startedAt) && service.items[0].startedAt >= 0,
    );
    service.dispose();
  }

  // --- generating: respond and answered tool calls turn it on ---------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const seen = [];
    service.onDidChangeGenerating((value) => seen.push(value));
    check("generating is off at construction", service.generating === false);
    wire.fire.session("s1");
    wire.fire.inputRequired("tok1");
    check("a wait opening leaves generating off", service.generating === false);
    wire.respondResult = false;
    service.respond("fails");
    check("a failed respond does not turn generating on", service.generating === false);
    wire.respondResult = true;
    service.respond("go");
    check("a successful respond turns generating on", service.generating === true);
    wire.fire.event("user_message", "go");
    wire.fire.event("tool_call", '[{"id":"c1","name":"fetch","arguments":{},"tool":"web/fetch"}]', {
      reply: 0,
    });
    check("a tool-call batch alone leaves generating as it was", service.generating === true);
    wire.fire.event("agent_message", "done", { reply: 1 });
    check("an agent_message turns generating off", service.generating === false);
    check("only real changes fired", isDeepStrictEqual(seen, [true, false]));
    service.dispose();
  }

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.session("s1");
    wire.fire.event("user_message", "first turn");
    wire.fire.event("tool_call", '[{"id":"c1","name":"ask","arguments":{},"tool":"user-input/ask"}]', {
      reply: 0,
    });
    wire.fire.inputRequired("tok1");
    check("an open wait keeps generating off", service.generating === false);
    wire.fire.event("tool_call_update", "operator answer", { tool_call_id: "c1" });
    check(
      "a tool_call_update answering a model batch call turns generating on",
      service.generating === true,
    );
    wire.fire.inputRequired("tok2");
    check("input_required turns generating off", service.generating === false);
    wire.fire.event("user_message", "second turn");
    wire.fire.event("tool_call_update", "stray", { tool_call_id: "c1" });
    check(
      "a tool_call_update whose call is in an earlier turn does not turn generating on",
      service.generating === false,
    );
    wire.fire.event("tool_call_update", "no id");
    check("a tool_call_update with no call id does not turn generating on", service.generating === false);
    wire.fire.event("tool_call", '[{"id":"c2","name":"read","arguments":{}}]', { reply: 1 });
    wire.fire.event("tool_call_update", "body", { tool_call_id: "c2" });
    check("a result for the current turn's call turns generating on", service.generating === true);
    wire.fire.error("server said no");
    check("an error turns generating off", service.generating === false);
    wire.fire.event("tool_call_update", "body", { tool_call_id: "c2" });
    check("an answered call turns it back on", service.generating === true);
    wire.fire.session("s1");
    check("a same-session acknowledgment leaves generating alone", service.generating === true);
    wire.fire.session("s2");
    check("an acknowledgment with a new session id turns generating off", service.generating === false);
    service.dispose();
  }

  // --- reconnecting: disconnect on, next acknowledgment off ------------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const seen = [];
    service.onDidChangeReconnecting((value) => seen.push(value));
    check("reconnecting is off at construction", service.reconnecting === false);
    wire.fire.disconnect();
    check("a disconnect turns reconnecting on", service.reconnecting === true);
    wire.fire.disconnect();
    wire.fire.session("s1");
    check("the next acknowledgment turns reconnecting off", service.reconnecting === false);
    wire.fire.disconnect();
    wire.fire.session("s1");
    check(
      "a same-session reattach acknowledgment also clears it",
      service.reconnecting === false && isDeepStrictEqual(seen, [true, false, true, false]),
    );
    service.dispose();
  }

  // --- ToolCallRow.tool: the batch entry's tool string, or null ---------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    wire.fire.event(
      "tool_call",
      JSON.stringify([
        { id: "a", name: "search", arguments: { query: "x" }, tool: "web/search" },
        { id: "b", name: "local", arguments: {} },
        { id: "c", name: "odd", arguments: {}, tool: 5 },
        { id: "d", name: "empty", arguments: {}, tool: "" },
        { id: "e", name: "nul", arguments: {}, tool: null },
      ]),
      { reply: 0 },
    );
    check(
      "a batch entry's tool string parses onto its row",
      isDeepStrictEqual(
        service.items[0].calls.map((row) => row.tool),
        ["web/search", null, null, null, null],
      ),
    );
    wire.fire.event("tool_call", "[1, null, [], \"x\"]", { reply: 1 });
    check("malformed entries produce no rows", service.items[1].calls.length === 0);
    service.dispose();
  }

  // --- cancelTurn: forwards, then settles the service's own view --------------

  {
    const wire = makeWire();
    const clock = { now: 10 };
    const service = new AgentSessionService(wire, () => clock.now);
    wire.fire.session("s1");
    wire.fire.inputRequired("tok1");
    service.respond("go");
    wire.fire.delta("reasoning", "thinking", 0);
    clock.now = 90;
    wire.respondResult = true;
    wire.cancelResult = false;
    check("a cancel on a down socket reports false", service.cancelTurn() === false);
    check(
      "a failed cancel leaves generating and the thinking open",
      service.generating === true && service.items[0].endedAt === null,
    );
    wire.cancelResult = true;
    let transcriptFires = 0;
    service.onDidChangeTranscript(() => transcriptFires++);
    check("a sent cancel reports true", service.cancelTurn() === true);
    check("the cancel frame went to the wire", wire.cancels === 2);
    check("a sent cancel turns generating off", service.generating === false);
    check("a sent cancel ends the open thinking", service.items[0].endedAt === 90);
    check("closing the thinking announced a transcript change", transcriptFires === 1);
    clock.now = 500;
    service.cancelTurn();
    check(
      "a second cancel does not move the ended thinking or announce a change",
      service.items[0].endedAt === 90 && transcriptFires === 1,
    );
    service.dispose();
  }

  // --- A reasoning chunk after Stop opens as an ended thought ------------------

  {
    const wire = makeWire();
    const clock = { now: 10 };
    const service = new AgentSessionService(wire, () => clock.now);
    const last = () => service.items[service.items.length - 1];
    wire.fire.session("s1");
    wire.fire.inputRequired("tok1");
    wire.respondResult = true;
    wire.cancelResult = true;
    service.respond("go");
    service.cancelTurn();
    clock.now = 40;
    wire.fire.delta("reasoning", "late", 0);
    check(
      "a first reasoning chunk after Stop opens an ended thought with no start",
      last().kind === "reasoning" &&
        last().pending === true &&
        last().startedAt === null &&
        last().endedAt === 40,
    );

    // The relaunched agent waiting on the operator ends the grace window.
    clock.now = 50;
    wire.fire.inputRequired("tok2");
    wire.fire.delta("reasoning", "after the wait", 1);
    check(
      "an input request after Stop makes the next first chunk stream",
      last().startedAt === 50 && last().endedAt === null,
    );

    // The operator answering restarts the turn's work.
    service.cancelTurn();
    service.respond("again");
    clock.now = 60;
    wire.fire.delta("reasoning", "fresh", 2);
    check(
      "an answer after Stop makes the next first chunk stream",
      last().startedAt === 60 && last().endedAt === null,
    );

    // A same-session reattach acknowledgment ends the grace window too.
    service.cancelTurn();
    wire.fire.session("s1");
    clock.now = 70;
    wire.fire.delta("reasoning", "reattached", 3);
    check(
      "a reattach acknowledgment after Stop makes the next first chunk stream",
      last().startedAt === 70 && last().endedAt === null,
    );

    // Deltas that append to an existing item never restamp it.
    clock.now = 80;
    wire.fire.delta("reasoning", " more", 3);
    check(
      "a later chunk appends without moving the clock stamps",
      last().text === "reattached more" && last().startedAt === 70 && last().endedAt === null,
    );
    service.dispose();
  }

  {
    const wire = makeWire();
    const clock = { now: 10 };
    const service = new AgentSessionService(wire, () => clock.now);
    wire.fire.session("s1");
    wire.fire.inputRequired("tok1");
    service.respond("go");
    wire.cancelResult = false;
    check("a cancel that fails to send reports false", service.cancelTurn() === false);
    clock.now = 20;
    wire.fire.delta("reasoning", "still thinking", 0);
    check(
      "a failed cancel does not end a new thought: the turn is not stopped",
      service.items[0].startedAt === 20 && service.items[0].endedAt === null,
    );
    service.dispose();
  }

  // --- Error titles: server errors have none; a downed socket is titled -------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    const heard = [];
    service.onError((message) => heard.push(message));
    wire.fire.error("unknown agent: bad");
    check("a server error carries no title", service.items[0].title === null);
    wire.fire.inputRequired("tok1");
    wire.respondResult = false;
    service.respond("hello");
    const local = service.items[service.items.length - 1];
    check(
      "a send on a down socket folds the titled connection error",
      local.kind === "error" &&
        local.title === "Connection failed" &&
        local.message ===
          "The connection was interrupted. Please check your network connection and try again.",
    );
    check(
      "the local failure announces the message as shown",
      heard[1] ===
        "The connection was interrupted. Please check your network connection and try again.",
    );
    service.dispose();
  }

  // --- Disposal severs the wire subscriptions -------------------------------

  {
    const wire = makeWire();
    const service = new AgentSessionService(wire);
    service.dispose();
    wire.fire.event("user_message", "after disposal");
    wire.fire.inputRequired("tok9");
    check(
      "a disposed service folds nothing",
      service.items.length === 0 && service.pendingInputToken === null,
    );
  }
});

if (failures.length > 0) {
  console.error(`agent-session-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-session-service: all assertions passed");
process.exit(0);
