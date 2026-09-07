import assert from "node:assert/strict";
import test from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const bundle = await esbuild.build({
  stdin: {
    contents: `
      export {
        createTakeRegistry,
        reduceTakeRegistry,
      } from "./src/ui/take-registry.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});

const { createTakeRegistry, reduceTakeRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

function context(start, end = start, original = "", compositionPrefix = "") {
  return {
    range: { start, end },
    original,
    compositionPrefix,
  };
}

function start(state, insertion) {
  return reduceTakeRegistry(state, { type: "user.start", context: insertion });
}

function stopAndCommit(state, eventId) {
  const stopping = reduceTakeRegistry(state, { type: "user.stop" });
  const stopEffect = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(stopEffect, "stop emits a capture request");
  const stopped = reduceTakeRegistry(stopping.state, {
    type: "capture.stopped",
    takeId: stopEffect.takeId,
    ok: true,
  });
  const commit = stopped.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "commit",
  );
  assert.ok(commit, "successful capture stop emits a commit");
  const sent = reduceTakeRegistry(stopped.state, {
    type: "wire.result",
    requestId: commit.requestId,
    eventId,
  });
  return { state: sent.state, effects: [...stopping.effects, ...stopped.effects, ...sent.effects] };
}

function server(state, event) {
  return reduceTakeRegistry(state, { type: "server.event", event });
}

function committed(itemId, eventId = `commit_${itemId}`) {
  return {
    type: "input_audio_buffer.committed",
    event_id: eventId,
    item_id: itemId,
    previous_item_id: null,
  };
}

function hypothesis(itemId, transcript, revision = 1) {
  return {
    type: "conversation.item.input_audio_transcription.hypothesis",
    event_id: `hypothesis_${itemId}_${revision}`,
    item_id: itemId,
    content_index: 0,
    revision,
    transcript,
    finalized: "",
    agreed: "",
    tentative: transcript,
    audio_start_ms: 0,
    audio_end_ms: 100,
  };
}

function completion(itemId, transcript) {
  return {
    type: "conversation.item.input_audio_transcription.completed",
    event_id: `completion_${itemId}`,
    item_id: itemId,
    content_index: 0,
    transcript,
    usage: { type: "duration", seconds: 0.1 },
  };
}

function editorReplacements(effects) {
  return effects.filter(
    (effect) => effect.domain === "editor" && effect.command === "replace",
  );
}

test("transition table replaces selections and gives completion authority", () => {
  let state = createTakeRegistry();
  const transitions = [
    {
      input: { type: "user.start", context: context(6, 10, "test") },
      replacements: [],
      takeCount: 1,
    },
    {
      input: { type: "server.event", event: hypothesis("selection", "spoken") },
      replacements: [{ from: 6, to: 10, text: "spoken" }],
      takeCount: 1,
    },
    {
      input: { type: "server.event", event: hypothesis("selection", "provisional", 2) },
      replacements: [{ from: 6, to: 12, text: "provisional" }],
      takeCount: 1,
    },
    {
      input: { type: "server.event", event: completion("selection", "final   ") },
      replacements: [{ from: 6, to: 17, text: "final" }],
      takeCount: 0,
    },
  ];

  for (const row of transitions) {
    const result = reduceTakeRegistry(state, row.input);
    assert.deepEqual(
      editorReplacements(result.effects).map(({ from, to, text }) => ({ from, to, text })),
      row.replacements,
    );
    assert.equal(result.state.takes.length, row.takeCount);
    state = result.state;
  }

  assert.ok(
    reduceTakeRegistry(state, {
      type: "server.event",
      event: hypothesis("selection", "late"),
    }).effects.length === 0,
    "a retired item cannot rewrite its completed selection",
  );
});

test("precommit binding confirms matches and rolls back mismatches", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  let result = server(state, hypothesis("provisional", "temporary"));
  state = result.state;
  assert.equal(state.takes[0].itemId, "provisional");

  state = stopAndCommit(state, "client_commit").state;
  result = server(state, committed("wrong"));
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 0,
      to: 9,
      text: "",
    },
  ]);
  assert.equal(result.state.takes.length, 0);
  assert.deepEqual(result.state.retiredItemIds.sort(), ["provisional", "wrong"]);
  assert.ok(
    result.effects.some(
      (effect) =>
        effect.domain === "status" &&
        effect.command === "local" &&
        effect.severity === "error",
    ),
  );

  state = start(result.state, context(0)).state;
  state = stopAndCommit(state, "client_commit_2").state;
  state = server(state, hypothesis("fresh", "new")).state;
  result = server(state, committed("fresh"));
  assert.equal(result.state.takes[0].itemId, "fresh");
  assert.equal(editorReplacements(result.effects).length, 0);
});

test("commit tombstones consume late acknowledgments without stealing a new take", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = stopAndCommit(state, "client_commit_1").state;
  state = reduceTakeRegistry(state, { type: "user.discard" }).state;

  state = start(state, context(0)).state;
  let result = server(state, committed("discarded"));
  assert.equal(result.state.takes[0].itemId, null);
  assert.ok(result.state.retiredItemIds.includes("discarded"));
  result = server(result.state, hypothesis("discarded", "WRONG TAKE"));
  assert.equal(editorReplacements(result.effects).length, 0);

  state = stopAndCommit(result.state, "client_commit_2").state;
  state = server(state, hypothesis("current", "right take")).state;
  result = server(state, committed("current"));
  assert.equal(result.state.takes[0].itemId, "current");
  assert.equal(result.state.takes[0].text, "right take");
});

test("duplicate acknowledgments preserve the next FIFO owner", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = stopAndCommit(state, "commit_a").state;
  state = server(state, committed("a")).state;
  state = start(state, context(0)).state;
  state = stopAndCommit(state, "commit_b").state;

  state = server(state, committed("a", "duplicate_a")).state;
  assert.deepEqual(state.awaitingCommit, [{ takeId: 2, itemId: null }]);
  const result = server(state, committed("b"));
  assert.deepEqual(
    result.state.takes.map((take) => take.itemId),
    ["a", "b"],
  );
});

test("decoded delta events accumulate into replacement snapshots", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  for (const [index, delta] of ["one", " two"].entries()) {
    const result = server(state, {
      type: "conversation.item.input_audio_transcription.delta",
      event_id: `delta_${index}`,
      item_id: "delta_item",
      content_index: 0,
      delta,
    });
    state = result.state;
    assert.equal(editorReplacements(result.effects)[0].text, index === 0 ? "one" : "one two");
  }
});

test("overlapping takes shift isolated regions and complete in reverse order", () => {
  let state = start(createTakeRegistry(), context(5)).state;
  state = stopAndCommit(state, "commit_a").state;
  let result = server(state, hypothesis("a", "first"));
  state = result.state;
  state = server(state, committed("a")).state;

  state = start(state, context(10, 10, "", " ")).state;
  state = stopAndCommit(state, "commit_b").state;
  result = server(state, hypothesis("b", "second"));
  state = server(result.state, committed("b")).state;
  assert.deepEqual(
    state.takes.map((take) => ({ itemId: take.itemId, from: take.from, text: take.text })),
    [
      { itemId: "a", from: 5, text: "first" },
      { itemId: "b", from: 10, text: " second" },
    ],
  );

  result = server(state, completion("b", "second"));
  state = result.state;
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 10,
      to: 17,
      text: " second",
    },
  ]);
  result = server(state, completion("a", "FIRST"));
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 5,
      to: 10,
      text: "FIRST",
    },
  ]);
  assert.equal(result.state.takes.length, 0);
});

test("rollback shifts later takes back and preserves their authority", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = stopAndCommit(state, "commit_a").state;
  state = server(state, hypothesis("a", "temporary")).state;
  state = server(state, committed("a")).state;
  state = start(state, context(9, 9, "", " ")).state;
  state = stopAndCommit(state, "commit_b").state;
  state = server(state, hypothesis("b", "kept")).state;
  state = server(state, committed("b")).state;

  let result = server(state, {
    type: "conversation.item.input_audio_transcription.failed",
    event_id: "failure_a",
    item_id: "a",
    content_index: 0,
    error: {
      type: "transcription_error",
      code: "failed",
      message: "must stay local",
    },
  });
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 0,
      to: 9,
      text: "",
    },
  ]);
  assert.equal(result.state.takes[0].from, 0);

  result = server(result.state, completion("b", "KEPT"));
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 0,
      to: 5,
      text: " KEPT",
    },
  ]);
});

test("sequential takes own exactly one composition separator", () => {
  let state = start(createTakeRegistry(), context(16, 16, "", " ")).state;
  state = stopAndCommit(state, "commit_first").state;
  state = server(state, committed("first")).state;
  let result = server(state, completion("first", "Second test beta"));
  assert.equal(editorReplacements(result.effects)[0].text, " Second test beta");

  state = start(result.state, context(32, 32, "", " ")).state;
  result = server(state, hypothesis("second", " leading"));
  assert.equal(editorReplacements(result.effects)[0].text, " leading");
  state = result.state;
  result = server(state, completion("second", "authoritative   "));
  assert.equal(editorReplacements(result.effects)[0].text, " authoritative");
});

test("a reconnect rolls back live state and rejects the old session's late events", () => {
  let state = start(createTakeRegistry(), context(4, 4, "", " ")).state;
  state = server(state, hypothesis("old", "temporary")).state;

  let result = reduceTakeRegistry(state, { type: "connection.lost" });
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 4,
      to: 14,
      text: "",
    },
  ]);
  assert.ok(
    result.effects.some(
      (effect) => effect.domain === "capture" && effect.command === "clear",
    ),
  );
  assert.ok(result.state.retiredItemIds.includes("old"));
  const stopEffect = result.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(stopEffect);
  state = reduceTakeRegistry(result.state, {
    type: "capture.stopped",
    takeId: stopEffect.takeId,
    ok: true,
  }).state;
  state = reduceTakeRegistry(state, { type: "connection.ready" }).state;
  assert.equal(state.connection, "ready");

  result = server(state, completion("old", "LATE"));
  assert.equal(result.effects.length, 0);
  state = start(result.state, context(4, 4, "", " ")).state;
  result = server(state, hypothesis("new", "fresh"));
  assert.equal(editorReplacements(result.effects)[0].text, " fresh");
});

test("capture and wire effects are typed and correlated to their take", () => {
  let result = start(createTakeRegistry(), context(0));
  let state = result.state;
  assert.deepEqual(result.effects, [
    { domain: "editor", command: "read-only", readOnly: true },
    { domain: "status", command: "recording", recording: true },
    {
      domain: "status",
      command: "local",
      label: "Listening...",
      severity: "info",
    },
  ]);

  result = reduceTakeRegistry(state, {
    type: "capture.audio",
    chunk: Uint8Array.from([1, 2]).buffer,
  });
  state = result.state;
  const append = result.effects[0];
  assert.equal(append.domain, "wire");
  assert.equal(append.command, "append");
  assert.equal(append.takeId, state.activeTakeId);

  result = reduceTakeRegistry(state, {
    type: "wire.result",
    requestId: append.requestId,
    eventId: "append_event",
  });
  state = result.state;
  const error = {
    type: "error",
    event_id: "server_error",
    error: {
      type: "invalid_request_error",
      code: "bad_audio",
      message: "must not surface",
      event_id: "append_event",
    },
  };
  result = server(state, error);
  assert.equal(result.state.takes.length, 0);
  assert.ok(
    result.effects.some(
      (effect) =>
        effect.domain === "status" &&
        effect.command === "local" &&
        !effect.label.includes("must not surface"),
    ),
  );
});

test("every transition preserves registry invariants without mutating its input", () => {
  let state = createTakeRegistry();
  const inputs = [
    { type: "user.start", context: context(0) },
    { type: "capture.audio", chunk: new ArrayBuffer(2) },
    { type: "user.stop" },
    { type: "connection.lost" },
    { type: "connection.ready" },
    { type: "user.start", context: context(0) },
    { type: "user.discard" },
  ];

  for (const input of inputs) {
    const before = structuredClone(state);
    const result = reduceTakeRegistry(state, input);
    assert.deepEqual(state, before, `${input.type} mutated its input`);
    assert.equal(
      new Set(result.state.takes.map((take) => take.id)).size,
      result.state.takes.length,
      `${input.type} duplicated a take id`,
    );
    assert.equal(
      result.state.takes.filter((take) => take.id === result.state.activeTakeId).length,
      result.state.activeTakeId === null ? 0 : 1,
      `${input.type} left an invalid active take`,
    );
    assert.equal(
      new Set(result.state.retiredItemIds).size,
      result.state.retiredItemIds.length,
      `${input.type} duplicated a tombstoned item id`,
    );
    for (let index = 1; index < result.state.takes.length; index += 1) {
      assert.ok(
        result.state.takes[index - 1].from <= result.state.takes[index].from,
        `${input.type} left take regions out of order`,
      );
    }
    state = result.state;
  }
});
