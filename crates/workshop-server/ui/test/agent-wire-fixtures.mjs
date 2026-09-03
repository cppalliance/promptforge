// The TS half of the agent-frame wire contract: every frame in the shared
// fixture crates/workshop-server/tests/fixtures/agent-frames.json
// routes through AgentSocket unchanged (server-to-client), and every frame
// the socket sends matches its fixture entry byte-for-byte as parsed JSON
// (client-to-server). The Rust half is the fixture test in
// src/protocol.rs; both suites pin the same case list, so a wire drift or
// a case added on one side fails the other.
// Run: node test/agent-wire-fixtures.mjs
import { readFile, writeFile } from "node:fs/promises";
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
      export { AgentSocket } from "./src/services/agent-socket.ts";
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

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-wire-fixtures-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, AgentSocket } = await import(pathToFileURL(bundlePath).href);

const fixture = JSON.parse(
  await readFile(path.join(testDir, "..", "..", "tests", "fixtures", "agent-frames.json"), "utf8"),
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Both suites pin exactly the same case list, so a case added on one side
// fails the other. This list is mirrored by the Rust fixture test.
const CASES = [
  "agent_delta_reasoning",
  "agent_delta_text",
  "agent_event_minimal",
  "agent_event_stamped",
  "agent_session",
  "agents",
  "attach",
  "cancel",
  "input_cancelled",
  "input_required",
  "input_response",
  "launch",
];
check(
  "the fixture holds exactly the cases both suites pin",
  isDeepStrictEqual(Object.keys(fixture).sort(), CASES),
);

const fakeSockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  sent = [];
  onopen = null;
  onclose = null;
  onerror = null;
  onmessage = null;
  constructor(url) {
    this.url = url;
    fakeSockets.push(this);
  }
  send(data) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
  }
  // Test-side controls, not part of the WebSocket surface.
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  message(frame) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}
globalThis.WebSocket = FakeWebSocket;

await assertNoLeaks(lifecycle, async () => {
  // --- Server-to-client: each fixture frame routes through unchanged ------

  const socket = new AgentSocket("ws://fake/agents/ws");
  const agents = [];
  const sessions = [];
  const events = [];
  const deltas = [];
  const required = [];
  const cancelled = [];
  socket.onAgents((list) => agents.push(list));
  socket.onSession((frame) => sessions.push(frame));
  socket.onEvent((frame) => events.push(frame));
  socket.onDelta((frame) => deltas.push(frame));
  socket.onInputRequired((token) => required.push(token));
  socket.onInputCancelled((token) => cancelled.push(token));
  socket.connect();
  const wire = fakeSockets[0];
  wire.open();

  wire.message(fixture.agents);
  wire.message(fixture.agent_session);
  wire.message(fixture.agent_event_minimal);
  wire.message(fixture.agent_event_stamped);
  wire.message(fixture.agent_delta_text);
  wire.message(fixture.agent_delta_reasoning);
  wire.message(fixture.input_required);
  wire.message(fixture.input_cancelled);

  check(
    "the agents fixture frame delivers its list verbatim",
    isDeepStrictEqual(agents, [fixture.agents.agents]),
  );
  check(
    "the agent_session fixture frame delivers verbatim",
    isDeepStrictEqual(sessions, [fixture.agent_session]),
  );
  check(
    "both agent_event fixture frames deliver verbatim, in order, index and reply intact",
    isDeepStrictEqual(events, [fixture.agent_event_minimal, fixture.agent_event_stamped]),
  );
  check(
    "the minimal event omits reply and the stamped event carries it, as the fixture does",
    events[0] !== undefined &&
      !("reply" in events[0]) &&
      events[1] !== undefined &&
      events[1].reply === fixture.agent_event_stamped.reply,
  );
  check(
    "both agent_delta fixture frames deliver verbatim with their superseding reply id",
    isDeepStrictEqual(deltas, [fixture.agent_delta_text, fixture.agent_delta_reasoning]),
  );
  check(
    "the input_required fixture frame delivers its token",
    isDeepStrictEqual(required, [fixture.input_required.token]),
  );
  check(
    "the input_cancelled fixture frame delivers its token",
    isDeepStrictEqual(cancelled, [fixture.input_cancelled.token]),
  );

  // --- Client-to-server: each send matches its fixture entry --------------

  socket.launch(fixture.launch.agent);
  socket.respond(fixture.input_response.token, fixture.input_response.text);
  socket.cancelTurn();
  check(
    "launch, input_response, and cancel sends match their fixture entries",
    isDeepStrictEqual(wire.sent, [fixture.launch, fixture.input_response, fixture.cancel]),
  );
  socket.dispose();

  const attacher = new AgentSocket("ws://fake/agents/ws");
  attacher.connect();
  fakeSockets[1].open();
  attacher.attach(fixture.attach.session);
  check(
    "an attach send matches its fixture entry",
    isDeepStrictEqual(fakeSockets[1].sent, [fixture.attach]),
  );
  attacher.dispose();
});

if (failures.length > 0) {
  console.error(`agent-wire-fixtures: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-wire-fixtures: all assertions passed");
process.exit(0);
