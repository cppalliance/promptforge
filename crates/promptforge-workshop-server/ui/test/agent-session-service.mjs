// The agent-session view model (src/services/agent-session.ts) against a
// scripted wire, no DOM: durable events fold into transcript items in
// log order; ephemeral deltas coalesce into pending items by their
// superseding reply id, per channel, and the durable event replaces
// them; late deltas after their round settled are dropped; the input pin
// follows input_required / input_cancelled / respond; a session
// acknowledgment resets the pin and a new session id resets the
// transcript; errors fold as items. Run: node test/agent-session-service.mjs
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
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { Emitter } from "./src/base/event.ts";
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
  };
  return {
    onAgents: emitters.agents.event,
    onSession: emitters.session.event,
    onEvent: emitters.event.event,
    onDelta: emitters.delta.event,
    onInputRequired: emitters.inputRequired.event,
    onInputCancelled: emitters.inputCancelled.event,
    onError: emitters.error.event,
    launched: [],
    responses: [],
    launchResult: true,
    respondResult: true,
    launch(agent) {
      this.launched.push(agent);
      return this.launchResult;
    },
    respond(token, text) {
      this.responses.push([token, text]);
      return this.respondResult;
    },
    fire: {
      agents: (list) => emitters.agents.fire(list),
      session: (session, agent = "chat") =>
        emitters.session.fire({ type: "agent_session", session, agent }),
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
      "the user item carries the byte-exact text",
      service.items[0].text === "hi there",
    );
    check(
      "reply and reasoning items carry their model label",
      service.items[1].model === "llama-3" && service.items[2].model === "llama-3",
    );
    check(
      "a settled durable item is not pending",
      service.items[1].pending === false && service.items[2].pending === false,
    );
    check(
      "the tool-call batch parses into one row per call",
      isDeepStrictEqual(service.items[3].calls, [
        { id: "call_1", name: "read", args: '{"path":"a"}' },
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
      "the response carried token and text",
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
    // starts must not carry it into its feed.
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
