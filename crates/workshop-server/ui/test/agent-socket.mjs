// Routing discipline of AgentSocket against a scripted fake WebSocket, no
// DOM needed: durable agent_event frames are cursor-deduplicated so an
// attach's replay from index zero delivers nothing twice; a reconnect
// reattaches to the acknowledged session automatically; sends report false
// when the socket is down; malformed frames are skipped without a throw;
// error frames deliver their message; disposal is never mistaken for a
// dropout. The frame shapes themselves are pinned by
// test/agent-wire-fixtures.mjs against the shared Rust fixture.
// Run: node test/agent-socket.mjs
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

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-socket-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, AgentSocket } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const fakeSockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  sent = [];
  closed = false;
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
    this.closed = true;
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
  raw(data) {
    this.onmessage?.({ data });
  }
  drop() {
    this.readyState = 3;
    this.onclose?.();
  }
}
globalThis.WebSocket = FakeWebSocket;

function event(index, content) {
  return {
    type: "agent_event",
    index,
    event: {
      kind: "user_message",
      section: "chat",
      chain_id: 0,
      depth: 0,
      turn: 0,
      content,
    },
  };
}

await assertNoLeaks(lifecycle, async () => {
  // --- The event cursor and the reattach replay ----------------------------

  const socket = new AgentSocket("ws://fake/agents/ws");
  const delivered = [];
  const sessions = [];
  let disconnects = 0;
  socket.onEvent((frame) => delivered.push(frame.index));
  socket.onSession((frame) => sessions.push(frame.session));
  socket.onDisconnect(() => disconnects++);
  socket.connect();
  fakeSockets[0].open();

  check("launch reports true on an open socket", socket.launch("chat") === true);
  fakeSockets[0].message({ type: "agent_session", session: "s1", agent: "chat" });
  fakeSockets[0].message(event(0, "one"));
  fakeSockets[0].message(event(1, "two"));
  check(
    "durable events deliver once each, in log order",
    isDeepStrictEqual(delivered, [0, 1]),
  );
  fakeSockets[0].message(event(0, "one"));
  check(
    "a duplicate index below the cursor is dropped, not re-delivered",
    isDeepStrictEqual(delivered, [0, 1]),
  );

  fakeSockets[0].drop();
  check("a dropout fires onDisconnect", disconnects === 1);

  // The dropout scheduled a backoff retry; connecting by hand stands in
  // for that timer so the test never waits on a real clock.
  socket.connect();
  const rewire = fakeSockets[1];
  rewire.open();
  check(
    "a reconnect reattaches to the acknowledged session by itself",
    isDeepStrictEqual(rewire.sent, [{ type: "attach", session: "s1" }]),
  );
  rewire.message({ type: "agent_session", session: "s1", agent: "chat" });
  check(
    "the reattach acknowledgment fires onSession again",
    isDeepStrictEqual(sessions, ["s1", "s1"]),
  );
  rewire.message(event(0, "one"));
  rewire.message(event(1, "two"));
  rewire.message(event(2, "three"));
  check(
    "the replay from index zero delivers only what the cursor has not seen",
    isDeepStrictEqual(delivered, [0, 1, 2]),
  );
  socket.dispose();
  check("disposal closes the live socket", rewire.closed === true);

  // --- A new session on the same socket resets the durable cursor ----------
  // The reachable path: the session dies while disconnected, the automatic
  // reattach is refused, and a fresh launch on the same socket starts a new
  // log at index zero - which must deliver, not fall below the old cursor.

  const relaunch = new AgentSocket("ws://fake/agents/ws");
  const relaunchDelivered = [];
  const refusals = [];
  relaunch.onEvent((frame) => relaunchDelivered.push(frame.index));
  relaunch.onError((message) => refusals.push(message));
  relaunch.connect();
  const doomed = fakeSockets[fakeSockets.length - 1];
  doomed.open();
  relaunch.launch("chat");
  doomed.message({ type: "agent_session", session: "old", agent: "chat" });
  doomed.message(event(0, "one"));
  doomed.message(event(1, "two"));
  doomed.drop();
  relaunch.connect();
  const fresh = fakeSockets[fakeSockets.length - 1];
  fresh.open();
  fresh.message({ type: "error", message: "unknown agent session" });
  check(
    "a refused reattach surfaces as the server's error",
    isDeepStrictEqual(refusals, ["unknown agent session"]),
  );
  relaunch.launch("chat");
  fresh.message({ type: "agent_session", session: "new", agent: "chat" });
  fresh.message(event(0, "fresh start"));
  check(
    "a new session's acknowledgment resets the cursor, so its log head delivers",
    isDeepStrictEqual(relaunchDelivered, [0, 1, 0]),
  );
  relaunch.dispose();

  // --- Sends report false when the socket is down --------------------------

  const down = new AgentSocket("ws://fake/agents/ws");
  check("launch reports false before a connect", down.launch("chat") === false);
  check("attach reports false before a connect", down.attach("s1") === false);
  check("respond reports false before a connect", down.respond("tok", "hi") === false);
  check("cancelTurn reports false before a connect", down.cancelTurn() === false);
  down.dispose();

  // --- Malformed and unknown frames are skipped without a throw ------------

  const tolerant = new AgentSocket("ws://fake/agents/ws");
  const heard = [];
  tolerant.onAgents((list) => heard.push(["agents", list]));
  tolerant.onEvent((frame) => heard.push(["event", frame.index]));
  tolerant.onInputRequired((token) => heard.push(["required", token]));
  tolerant.onError((message) => heard.push(["error", message]));
  tolerant.connect();
  const noisy = fakeSockets[fakeSockets.length - 1];
  noisy.open();
  noisy.raw("not json");
  noisy.message({ type: "mystery" });
  noisy.message({ type: "agent_event", event: {} });
  noisy.message({ type: "input_required" });
  noisy.message({ type: "agents", agents: "not a list" });
  check(
    "malformed frames are skipped; a listless agents push degrades to empty",
    isDeepStrictEqual(heard, [["agents", []]]),
  );

  // --- Error frames deliver their message, with a fallback -----------------

  noisy.message({ type: "error", message: "unknown agent session" });
  noisy.message({ type: "error", message: "" });
  check(
    "error frames deliver the server's message, or the fallback when empty",
    isDeepStrictEqual(heard.slice(1), [
      ["error", "unknown agent session"],
      ["error", "the agent session failed"],
    ]),
  );
  tolerant.dispose();

  // --- Disposal is never mistaken for a dropout -----------------------------

  const torn = new AgentSocket("ws://fake/agents/ws");
  let tornDisconnects = 0;
  torn.onDisconnect(() => tornDisconnects++);
  torn.connect();
  const last = fakeSockets[fakeSockets.length - 1];
  last.open();
  torn.dispose();
  last.drop();
  check(
    "a close after disposal fires no disconnect and schedules no reconnect",
    tornDisconnects === 0,
  );
});

if (failures.length > 0) {
  console.error(`agent-socket: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-socket: all assertions passed");
process.exit(0);
