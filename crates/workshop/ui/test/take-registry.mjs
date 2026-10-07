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
      } from "./src/parts/take/take-registry.ts";
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

function parts(itemId, revision, { finalized = "", agreed = "", tentative = "" }) {
  return {
    type: "conversation.item.input_audio_transcription.hypothesis",
    event_id: `hypothesis_${itemId}_${revision}`,
    item_id: itemId,
    content_index: 0,
    revision,
    transcript: `${finalized}${agreed}${tentative}`,
    finalized,
    agreed,
    tentative,
    audio_start_ms: 0,
    audio_end_ms: 100,
  };
}

// Agreed text renders whole; the tentative hold-back has its own tests.
function hypothesis(itemId, transcript, revision = 1) {
  return parts(itemId, revision, { agreed: transcript });
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

function patches(effects) {
  return editorReplacements(effects).map(({ from, to, text }) => ({ from, to, text }));
}

function tentativeMarks(effects) {
  return effects
    .filter((effect) => effect.domain === "editor" && effect.command === "tentative")
    .map(({ takeId, range }) => ({ takeId, range }));
}

function applyEdits(text, effects) {
  let next = text;
  for (const { from, to, text: inserted } of editorReplacements(effects)) {
    assert.ok(from >= 0 && from <= to && to <= next.length, `replace [${from}, ${to}] lies inside the text`);
    next = next.slice(0, from) + inserted + next.slice(to);
  }
  return next;
}

// Every target leaves the caret after a replace's insert and moves it on a caret effect.
function applyToTarget(target, effects) {
  let caret = target.caret;
  for (const effect of effects) {
    if (effect.domain === "editor" && effect.command === "replace") {
      caret = effect.from + effect.text.length;
    } else if (effect.domain === "editor" && effect.command === "caret") {
      caret = effect.at;
    }
  }
  return { text: applyEdits(target.text, effects), caret };
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
      replacements: [
        { from: 6, to: 17, text: "test" },
        { from: 6, to: 10, text: "final" },
      ],
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
      transient: true,
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

test("hypotheses at or below their item's last applied revision are ignored", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = stopAndCommit(state, "commit_a").state;
  let result = server(state, hypothesis("a", "ask not", 2));
  assert.equal(editorReplacements(result.effects)[0].text, "ask not");
  state = server(result.state, committed("a")).state;

  for (const revision of [2, 1, 0]) {
    const stale = server(state, hypothesis("a", `stale ${revision}`, revision));
    assert.deepEqual(stale.effects, [], `revision ${revision} emits no effects`);
    assert.deepEqual(stale.state, state, `revision ${revision} leaves the registry unchanged`);
  }

  result = server(state, hypothesis("a", "ask not what", 5));
  assert.deepEqual(patches(result.effects), [{ from: 7, to: 7, text: " what" }]);
  state = result.state;
  assert.deepEqual(server(state, hypothesis("a", "stale", 4)).effects, []);

  state = start(state, context(12, 12, "", " ")).state;
  result = server(state, hypothesis("b", "your", 1));
  assert.deepEqual(
    editorReplacements(result.effects).map(({ from, to, text }) => ({ from, to, text })),
    [{ from: 12, to: 12, text: " your" }],
    "another item's revisions are tracked separately",
  );
  state = result.state;
  assert.deepEqual(server(state, hypothesis("b", "stale", 1)).effects, []);
  result = server(state, hypothesis("a", "ask not what you", 6));
  assert.deepEqual(patches(result.effects), [{ from: 12, to: 12, text: " you" }]);
  assert.deepEqual(
    result.state.takes.map(({ itemId, text }) => ({ itemId, text })),
    [
      { itemId: "a", text: "ask not what you" },
      { itemId: "b", text: " your" },
    ],
  );
});

test("a revision replaces only the word runs that changed and shifts later takes", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = stopAndCommit(state, "commit_a").state;
  let result = server(state, hypothesis("a", "ask not what"));
  let editor = applyEdits("", result.effects);
  state = server(result.state, committed("a")).state;
  state = start(state, context(12, 12, "", " ")).state;
  result = server(state, hypothesis("b", "your"));
  editor = applyEdits(editor, result.effects);
  state = result.state;

  result = server(state, hypothesis("a", "Ask not, what you", 2));
  assert.deepEqual(
    patches(result.effects),
    [
      { from: 0, to: 7, text: "Ask not," },
      { from: 13, to: 13, text: " you" },
    ],
    "the unchanged word between the two changes is not rewritten",
  );
  editor = applyEdits(editor, result.effects);
  assert.equal(editor, "Ask not, what you your");
  assert.deepEqual(
    result.state.takes.map(({ from, to, text }) => ({ from, to, text })),
    [
      { from: 0, to: 17, text: "Ask not, what you" },
      { from: 17, to: 22, text: " your" },
    ],
  );

  assert.deepEqual(
    patches(server(result.state, hypothesis("a", "Ask not, what you", 3)).effects),
    [],
    "an unchanged revision writes nothing",
  );
});

test("an interior-only correction leaves the caret at the take's end for the next take", () => {
  let target = { text: "", caret: 0 };
  let result = server(start(createTakeRegistry(), context(0)).state, hypothesis("a", "ask not what"));
  target = applyToTarget(target, result.effects);

  result = server(result.state, hypothesis("a", "ask NOT what", 2));
  assert.deepEqual(patches(result.effects), [{ from: 4, to: 7, text: "NOT" }]);
  target = applyToTarget(target, result.effects);
  assert.deepEqual(target, { text: "ask NOT what", caret: 12 }, "a hypothesis patch parks the caret");

  let state = stopAndCommit(result.state, "commit_a").state;
  state = server(state, committed("a")).state;
  result = server(state, completion("a", "Ask NOT what"));
  assert.deepEqual(patches(result.effects), [
    { from: 0, to: 12, text: "" },
    { from: 0, to: 0, text: "Ask NOT what" },
  ]);
  target = applyToTarget(target, result.effects);
  assert.deepEqual(target, { text: "Ask NOT what", caret: 12 }, "a landed completion parks the caret");

  state = start(result.state, context(target.caret, target.caret, "", " ")).state;
  result = server(state, hypothesis("b", "Second"));
  assert.equal(applyToTarget(target, result.effects).text, "Ask NOT what Second");
});

test("a changed middle past the alignment budget becomes one patch that lands the exact text", () => {
  const words = (count) => Array.from({ length: count }, (_, index) => `w${index}`).join(" ");
  const revise = (count) => {
    const before = `keep start alpha ${words(count)} omega keep end`;
    const after = `keep start ALPHA ${words(count)} OMEGA keep end`;
    const state = server(start(createTakeRegistry(), context(0)).state, hypothesis("a", before)).state;
    const { effects } = server(state, hypothesis("a", after, 2));
    assert.equal(applyEdits(before, effects), after, `${count} middle words land the exact text`);
    return { before, after, patches: patches(effects) };
  };

  assert.equal(revise(10).patches.length, 2, "a middle inside the budget patches each changed word");

  const { before, after, patches: [patch, ...rest] } = revise(150);
  assert.deepEqual(rest, [], "about 300 runs a side collapse into one patch");
  const prefix = "keep start ".length;
  const suffix = " keep end".length;
  assert.deepEqual(
    patch,
    { from: prefix, to: before.length - suffix, text: after.slice(prefix, after.length - suffix) },
    "the patch spans exactly the changed middle and keeps the shared prefix and suffix",
  );
});

test("the tentative mark covers exactly the shown tentative words and clears on completion", () => {
  let editor = "Note";
  let state = start(createTakeRegistry(), context(4, 4, "", " ")).state;
  const step = (event) => {
    const result = server(state, event);
    state = result.state;
    editor = applyEdits(editor, result.effects);
    return tentativeMarks(result.effects);
  };

  assert.deepEqual(
    step(parts("a", 1, { tentative: "ask not" })),
    [{ takeId: 1, range: { from: 5, to: 8 } }],
    "the mark leaves out the take's separator",
  );
  assert.equal(editor, "Note ask");

  assert.deepEqual(
    step(parts("a", 2, { agreed: "ask not", tentative: " what your" })),
    [{ takeId: 1, range: { from: 12, to: 17 } }],
  );
  assert.equal(editor, "Note ask not what");
  assert.equal(editor.slice(12, 17), " what");

  assert.deepEqual(
    step(parts("a", 3, { agreed: "ask not what", tentative: " your" })),
    [{ takeId: 1, range: null }],
    "a tail that is wholly held back clears the mark",
  );
  assert.equal(editor, "Note ask not what");

  assert.deepEqual(
    step(parts("a", 4, { agreed: "ask not what", tentative: " your country" })),
    [{ takeId: 1, range: { from: 17, to: 22 } }],
  );

  state = stopAndCommit(state, "commit_a").state;
  state = server(state, committed("a")).state;
  assert.deepEqual(
    step(completion("a", "ask not what your country")),
    [{ takeId: 1, range: null }],
    "completion clears the mark",
  );
  assert.equal(editor, "Note ask not what your country");
});

test("a discarded, failed, or disconnected take clears its tentative mark once", () => {
  const recording = start(createTakeRegistry(), context(0)).state;
  const live = server(recording, parts("live", 1, { agreed: "kept", tentative: " words here" }));
  assert.deepEqual(tentativeMarks(live.effects), [{ takeId: 1, range: { from: 4, to: 10 } }]);

  for (const [label, input] of [
    ["a discard", { type: "user.discard" }],
    ["a transcription failure", { type: "server.event", event: transcriptionFailure("live") }],
    ["a connection loss", { type: "connection.lost" }],
  ]) {
    assert.deepEqual(
      tentativeMarks(reduceTakeRegistry(live.state, input).effects),
      [{ takeId: 1, range: null }],
      `${label} clears the mark once`,
    );
  }
});

test("the last tentative word stays hidden until the next update or completion", () => {
  let editor = "";
  let state = start(createTakeRegistry(), context(0)).state;
  const show = (event) => {
    const result = server(state, event);
    state = result.state;
    editor = applyEdits(editor, result.effects);
    return editor;
  };

  assert.equal(show(parts("a", 1, { tentative: "ask" })), "", "a lone tentative word is held back");
  assert.equal(
    show(parts("a", 2, { tentative: "ask not" })),
    "ask",
    "the next update shows the word it held back",
  );
  assert.equal(show(parts("a", 3, { agreed: "ask not", tentative: " what" })), "ask not");
  assert.equal(
    show(parts("a", 4, { agreed: "ask not wh", tentative: "at" })),
    "ask not what",
    "a fragment glued to stable text is not a whole word and stays shown",
  );

  state = stopAndCommit(state, "commit_a").state;
  state = server(state, committed("a")).state;
  assert.equal(show(completion("a", "ask not what you")), "ask not what you", "completion shows every word");
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
    assert.equal(state.takes[0].text, index === 0 ? "one" : "one two");
    assert.equal(editorReplacements(result.effects)[0].text, index === 0 ? "one" : " two");
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
  assert.deepEqual(
    patches(result.effects),
    [
      { from: 10, to: 17, text: "" },
      { from: 10, to: 10, text: " second" },
    ],
    "an unchanged completion still lands as one undoable write",
  );
  assert.equal(state.takes.length, 1);
  result = server(state, completion("a", "FIRST"));
  assert.deepEqual(editorReplacements(result.effects), [
    { domain: "editor", command: "replace", from: 5, to: 10, text: "", transient: true },
    { domain: "editor", command: "replace", from: 5, to: 5, text: "FIRST", transient: false },
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
  assert.deepEqual(patches(result.effects), [
    { from: 9, to: 14, text: "" },
    { from: 9, to: 9, text: " KEPT" },
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
  assert.deepEqual(
    patches(result.effects),
    [
      { from: 32, to: 40, text: "" },
      { from: 32, to: 32, text: " authoritative" },
    ],
    "the landed completion keeps the take's one separator",
  );
});

test("a reconnect lands agreed text and rejects the old session's late events", () => {
  let state = start(createTakeRegistry(), context(4, 4, "", " ")).state;
  state = server(state, hypothesis("old", "temporary")).state;

  let result = reduceTakeRegistry(state, { type: "connection.lost" });
  assert.deepEqual(editorReplacements(result.effects), [
    { domain: "editor", command: "replace", from: 4, to: 14, text: "", transient: true },
    { domain: "editor", command: "replace", from: 4, to: 4, text: " temporary", transient: false },
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
