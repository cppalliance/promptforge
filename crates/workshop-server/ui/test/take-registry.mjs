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

function transcriptionFailure(itemId, eventId = `failure_${itemId}`) {
  return {
    type: "conversation.item.input_audio_transcription.failed",
    event_id: eventId,
    item_id: itemId,
    content_index: 0,
    error: {
      type: "server_error",
      code: "precommit_transcription_failed",
      message: "must stay local",
      param: null,
    },
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
  assert.deepEqual(
    result.state.retiredItems.map(({ itemId }) => itemId).sort(),
    ["provisional", "wrong"],
  );
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
  assert.ok(result.state.retiredItems.some(({ itemId }) => itemId === "discarded"));
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
  assert.deepEqual(state.awaitingCommit, [
    { generation: 0, takeId: 2, itemId: null },
  ]);
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

test("terminal failure preserves visible text and later take coordinates", () => {
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
  assert.deepEqual(editorReplacements(result.effects), []);
  assert.equal(result.state.takes[0].from, 9);

  result = server(result.state, completion("b", "KEPT"));
  assert.deepEqual(editorReplacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 9,
      to: 14,
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
  assert.deepEqual(result.state.retiredItems, []);
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

test("retained-audio overload stops capture and commits accepted visible text", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  let result = server(state, hypothesis("long_take", "accepted visible words"));
  state = result.state;
  for (let stride = 0; stride < 360; stride++) {
    const appendResult = reduceTakeRegistry(state, {
      type: "capture.audio",
      chunk: Uint8Array.from([1, 0]).buffer,
    });
    const append = appendResult.effects.find(
      (effect) => effect.domain === "wire" && effect.command === "append",
    );
    assert.ok(append);
    state = reduceTakeRegistry(appendResult.state, {
      type: "wire.result",
      requestId: append.requestId,
      eventId: `hour_append_${stride}`,
    }).state;
    assert.ok(state.clientEvents.length <= 1, "append correlation history stays fixed");
  }
  assert.deepEqual(state.clientEvents, [
    {
      eventId: "hour_append_359",
      generation: 0,
      takeId: state.activeTakeId,
      command: "append",
    },
  ]);

  result = server(state, {
    type: "error",
    event_id: "stale_server_overload",
    error: {
      type: "overload_error",
      code: "too_much_unfinalized_audio",
      message: "must stay local",
      param: "audio",
      event_id: "hour_append_0",
    },
  });
  assert.equal(result.state.capture, "recording", "retired append ownership cannot stop its take");
  state = result.state;

  result = server(state, {
    type: "error",
    event_id: "server_overload",
    error: {
      type: "overload_error",
      code: "too_much_unfinalized_audio",
      message: "must stay local",
      param: "audio",
      event_id: "hour_append_359",
    },
  });
  const duplicate = server(result.state, {
    type: "error",
    event_id: "duplicate_server_overload",
    error: {
      type: "overload_error",
      code: "too_much_unfinalized_audio",
      message: "must stay local",
      param: "audio",
      event_id: "hour_append_359",
    },
  });

  assert.equal(result.state.takes[0].text, "accepted visible words");
  assert.equal(result.state.capture, "stopping");
  assert.equal(
    [...result.effects, ...duplicate.effects].filter(
      (effect) => effect.domain === "capture" && effect.command === "stop",
    ).length,
    1,
    "duplicate overload emits one capture stop",
  );
  assert.equal(
    result.effects.some(
      (effect) =>
        (effect.domain === "capture" && effect.command === "clear") ||
        (effect.domain === "wire" && effect.command === "clear") ||
        (effect.domain === "editor" && effect.command === "replace"),
    ),
    false,
    "throughput overload cannot erase accepted visible text",
  );

  const stopped = reduceTakeRegistry(duplicate.state, {
    type: "capture.stopped",
    takeId: result.state.stoppingTakeId,
    ok: true,
  });
  assert.ok(
    stopped.effects.some(
      (effect) => effect.domain === "wire" && effect.command === "commit",
    ),
    "the still-valid accepted input commits after capture flushes",
  );
});

test("precommit and terminal transcription failures preserve visible editor text", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = server(state, hypothesis("failed_take", "accepted visible words")).state;
  let result = reduceTakeRegistry(state, {
    type: "capture.audio",
    chunk: Uint8Array.from([1, 0]).buffer,
  });
  const append = result.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "append",
  );
  assert.ok(append);
  state = reduceTakeRegistry(result.state, {
    type: "wire.result",
    requestId: append.requestId,
    eventId: "failed_append",
  }).state;
  const precommit = {
    type: "error",
    event_id: "precommit_error",
    error: {
      type: "invalid_request_error",
      code: "precommit_transcription_failed",
      message: "must stay local",
      param: "audio",
      event_id: "failed_append",
    },
  };

  result = server(state, precommit);
  assert.equal(result.state.capture, "stopping");
  assert.equal(result.state.takes[0].text, "accepted visible words");
  assert.equal(editorReplacements(result.effects).length, 0);
  assert.equal(
    result.effects.some(
      (effect) =>
        (effect.domain === "capture" && effect.command === "clear") ||
        (effect.domain === "wire" && effect.command === "clear"),
    ),
    false,
  );
  assert.equal(
    result.effects.filter(
      (effect) => effect.domain === "capture" && effect.command === "stop",
    ).length,
    1,
  );
  const duplicatePrecommit = server(result.state, precommit);
  assert.deepEqual(duplicatePrecommit.effects, []);

  const stopped = reduceTakeRegistry(duplicatePrecommit.state, {
    type: "capture.stopped",
    takeId: result.state.stoppingTakeId,
    ok: true,
  });
  const commit = stopped.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "commit",
  );
  assert.ok(commit);
  state = reduceTakeRegistry(stopped.state, {
    type: "wire.result",
    requestId: commit.requestId,
    eventId: "failed_commit",
  }).state;
  state = server(state, committed("failed_take")).state;

  result = server(state, transcriptionFailure("failed_take"));
  assert.equal(result.state.takes.length, 0);
  assert.ok(
    result.state.retiredItems.some(({ itemId }) => itemId === "failed_take"),
  );
  assert.equal(editorReplacements(result.effects).length, 0);
  assert.ok(
    result.effects.some(
      (effect) =>
        effect.domain === "editor" &&
        effect.command === "read-only" &&
        effect.readOnly === false,
    ),
  );
  assert.ok(
    result.effects.some(
      (effect) =>
        effect.domain === "status" &&
        effect.command === "local" &&
        effect.label ===
          "Dictation could not be fully transcribed. Visible text was kept and can be edited." &&
        effect.severity === "error",
    ),
  );

  const duplicateFailure = server(
    result.state,
    transcriptionFailure("failed_take", "duplicate_failure"),
  );
  assert.deepEqual(duplicateFailure.effects, []);
  assert.deepEqual(server(duplicateFailure.state, hypothesis("failed_take", "late")).effects, []);
  assert.deepEqual(
    server(duplicateFailure.state, completion("failed_take", "LATE")).effects,
    [],
  );
});

test("precommit recovery requires the active take's latest append correlation", () => {
  function precommit(eventId, includeCorrelation = true) {
    const error = {
      type: "invalid_request_error",
      code: "precommit_transcription_failed",
      message: "must stay local",
      param: "audio",
    };
    if (includeCorrelation) {
      error.event_id = eventId;
    }
    return {
      type: "error",
      event_id: `server_${String(eventId)}`,
      error,
    };
  }

  let state = start(createTakeRegistry(), context(0)).state;
  state = server(state, hypothesis("retired_take", "visible old text")).state;
  let append = reduceTakeRegistry(state, {
    type: "capture.audio",
    chunk: new ArrayBuffer(2),
  });
  const retiredRequest = append.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "append",
  );
  assert.ok(retiredRequest);
  state = reduceTakeRegistry(append.state, {
    type: "wire.result",
    requestId: retiredRequest.requestId,
    eventId: "retired_append",
  }).state;
  const terminal = server(state, transcriptionFailure("retired_take"));
  const terminalStop = terminal.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(terminalStop);
  state = reduceTakeRegistry(terminal.state, {
    type: "capture.stopped",
    takeId: terminalStop.takeId,
    ok: true,
  }).state;

  state = start(state, context(16)).state;
  append = reduceTakeRegistry(state, {
    type: "capture.audio",
    chunk: new ArrayBuffer(2),
  });
  const supersededRequest = append.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "append",
  );
  assert.ok(supersededRequest);
  state = reduceTakeRegistry(append.state, {
    type: "wire.result",
    requestId: supersededRequest.requestId,
    eventId: "superseded_append",
  }).state;
  append = reduceTakeRegistry(state, {
    type: "capture.audio",
    chunk: new ArrayBuffer(2),
  });
  const liveRequest = append.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "append",
  );
  assert.ok(liveRequest);
  state = reduceTakeRegistry(append.state, {
    type: "wire.result",
    requestId: liveRequest.requestId,
    eventId: "live_append",
  }).state;

  for (const stale of [
    precommit(null),
    precommit(undefined, false),
    precommit("retired_append"),
    precommit("superseded_append"),
  ]) {
    const ignored = server(state, stale);
    assert.equal(ignored.state.capture, "recording");
    assert.deepEqual(ignored.effects, []);
    state = ignored.state;
  }

  const matched = server(state, precommit("live_append"));
  assert.equal(matched.state.capture, "stopping");
  assert.equal(
    matched.effects.filter(
      (effect) => effect.domain === "capture" && effect.command === "stop",
    ).length,
    1,
  );
  assert.deepEqual(server(matched.state, precommit("live_append")).effects, []);
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
      new Set(
        result.state.retiredItems.map(
          ({ generation, itemId }) => `${generation}:${itemId}`,
        ),
      ).size,
      result.state.retiredItems.length,
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

test("connection generations scope wire identity and permit immediate item ID reuse", () => {
  let state = reduceTakeRegistry(createTakeRegistry(), {
    type: "connection.ready",
    generation: 1,
  }).state;
  state = reduceTakeRegistry(state, {
    type: "user.start",
    generation: 1,
    context: context(0),
  }).state;
  state = reduceTakeRegistry(state, {
    type: "server.event",
    generation: 1,
    event: hypothesis("reused", "old words"),
  }).state;

  let result = reduceTakeRegistry(state, {
    type: "connection.lost",
    generation: 1,
  });
  assert.equal(result.state.activeGeneration, 1);
  state = reduceTakeRegistry(result.state, {
    type: "connection.ready",
    generation: 2,
  }).state;
  state = reduceTakeRegistry(state, {
    type: "user.start",
    generation: 2,
    context: context(0),
  }).state;
  result = reduceTakeRegistry(state, {
    type: "server.event",
    generation: 2,
    event: hypothesis("reused", "fresh words"),
  });
  assert.equal(result.state.takes[0].text, "fresh words");
  assert.equal(result.state.takes[0].itemGeneration, 2);
  assert.equal(result.state.takes[0].itemId, "reused");

  const freshRecording = result.state;
  const idle = reduceTakeRegistry(createTakeRegistry(), {
    type: "connection.ready",
    generation: 2,
  }).state;
  const stopping = reduceTakeRegistry(freshRecording, {
    type: "user.stop",
    generation: 2,
  });
  const stopEffect = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(stopEffect);
  const appending = reduceTakeRegistry(freshRecording, {
    type: "capture.audio",
    generation: 2,
    chunk: new ArrayBuffer(2),
  });
  const appendEffect = appending.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "append",
  );
  assert.ok(appendEffect);
  const unavailable = reduceTakeRegistry(freshRecording, {
    type: "connection.lost",
    generation: 2,
  }).state;

  const staleCases = [
    {
      label: "connection readiness",
      state: unavailable,
      input: { type: "connection.ready", generation: 1 },
    },
    {
      label: "connection loss",
      state: freshRecording,
      input: { type: "connection.lost", generation: 1 },
    },
    {
      label: "user start",
      state: idle,
      input: {
        type: "user.start",
        generation: 1,
        context: context(0),
      },
    },
    {
      label: "user stop",
      state: freshRecording,
      input: { type: "user.stop", generation: 1 },
    },
    {
      label: "user discard",
      state: freshRecording,
      input: { type: "user.discard", generation: 1 },
    },
    {
      label: "capture audio",
      state: freshRecording,
      input: { type: "capture.audio", generation: 1, chunk: new ArrayBuffer(2) },
    },
    {
      label: "capture completion",
      state: stopping.state,
      input: {
        type: "capture.stopped",
        generation: 1,
        takeId: stopEffect.takeId,
        ok: true,
      },
    },
    {
      label: "wire result",
      state: appending.state,
      input: {
        type: "wire.result",
        generation: 1,
        requestId: appendEffect.requestId,
        eventId: "stale",
      },
    },
    {
      label: "service error",
      state: freshRecording,
      input: { type: "service.error", generation: 1, eventId: null },
    },
    {
      label: "server event",
      state: freshRecording,
      input: {
        type: "server.event",
        generation: 1,
        event: completion("reused", "STALE"),
      },
    },
  ];

  for (const { label, state: current, input } of staleCases) {
    const before = structuredClone(current);
    const accepted = reduceTakeRegistry(current, { ...input, generation: 2 });
    assert.notDeepEqual(
      { state: accepted.state, effects: accepted.effects },
      { state: before, effects: [] },
      `${label} fixture must exercise behavior behind the generation guard`,
    );

    const ignored = reduceTakeRegistry(current, input);
    assert.deepEqual(ignored.state, before, `${label} from the old socket mutated state`);
    assert.deepEqual(ignored.effects, [], `${label} from the old socket emitted effects`);
  }
});
