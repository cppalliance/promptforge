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
      } from "./src/ui/take/take-registry.ts";
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

function reduce(state, input) {
  return reduceTakeRegistry(state, input);
}

function start(state, insertion) {
  return reduce(state, { type: "user.start", context: insertion });
}

function stop(state) {
  return reduce(state, { type: "user.stop" });
}

function finishCapture(state, takeId, ok = true) {
  return reduce(state, { type: "capture.stopped", takeId, ok });
}

function stopAndCommit(state, eventId) {
  const stopping = stop(state);
  const stopEffect = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(stopEffect);
  const stopped = finishCapture(stopping.state, stopEffect.takeId);
  const commit = stopped.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "commit",
  );
  assert.ok(commit);
  return reduce(stopped.state, {
    type: "wire.result",
    requestId: commit.requestId,
    eventId,
  });
}

function server(state, event) {
  return reduce(state, { type: "server.event", event });
}

function committed(itemId) {
  return {
    type: "input_audio_buffer.committed",
    event_id: `commit_${itemId}`,
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

function replacements(effects) {
  return effects.filter(
    (effect) => effect.domain === "editor" && effect.command === "replace",
  );
}

test("captured coordinate width owns replacement and rollback independently of text length", () => {
  let state = start(createTakeRegistry(), context(4, 9, "xy")).state;

  let result = server(state, hypothesis("selection", "spoken"));
  assert.deepEqual(replacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 4,
      to: 9,
      text: "spoken",
    },
  ]);
  state = result.state;

  result = reduce(state, { type: "user.discard" });
  assert.deepEqual(replacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 4,
      to: 10,
      text: "xy",
    },
  ]);
});

test("a precommit tombstone consumes its matching acknowledgment before the next take", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  state = server(state, hypothesis("discarded", "temporary")).state;
  state = stopAndCommit(state, "client_discarded").state;
  state = reduce(state, { type: "user.discard" }).state;

  state = start(state, context(0)).state;
  let result = server(state, committed("discarded"));
  assert.equal(result.state.awaitingCommit.length, 0);
  assert.equal(result.state.takes[0].itemId, null);

  state = stopAndCommit(result.state, "client_current").state;
  state = server(state, committed("current")).state;
  assert.equal(state.takes[0].itemId, "current");

  result = server(state, hypothesis("current", "provisional"));
  assert.equal(result.state.takes[0].text, "provisional");
  result = server(result.state, completion("current", "complete"));
  assert.deepEqual(replacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 0,
      to: 11,
      text: "complete",
    },
  ]);
  assert.equal(result.state.takes.length, 0);
});

test("a mismatched capture completion cannot release the stopping take", () => {
  const recording = start(createTakeRegistry(), context(0)).state;
  const stopping = stop(recording);
  const owner = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(owner);

  const before = structuredClone(stopping.state);
  const stale = finishCapture(stopping.state, owner.takeId + 100);
  assert.deepEqual(stale.state, before);
  assert.deepEqual(stale.effects, []);

  const owned = finishCapture(stale.state, owner.takeId);
  assert.equal(owned.state.capture, "idle");
  assert.ok(
    owned.effects.some(
      (effect) => effect.domain === "wire" && effect.command === "commit",
    ),
  );
});

test("a duplicate capture completion cannot recommit an older retained take", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  let stopping = stop(state);
  const firstOwner = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(firstOwner);
  state = finishCapture(stopping.state, firstOwner.takeId).state;

  state = start(state, context(0)).state;
  stopping = stop(state);
  const secondOwner = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(secondOwner);

  const before = structuredClone(stopping.state);
  const duplicate = finishCapture(stopping.state, firstOwner.takeId);
  assert.deepEqual(duplicate.state, before);
  assert.deepEqual(duplicate.effects, []);

  const owned = finishCapture(duplicate.state, secondOwner.takeId);
  const commits = owned.effects.filter(
    (effect) => effect.domain === "wire" && effect.command === "commit",
  );
  assert.equal(commits.length, 1);
  assert.equal(commits[0].takeId, secondOwner.takeId);
});

test("audio flushed while capture stops remains owned by the stopping take", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  const stopping = stop(state);
  const owner = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(owner);

  const flushed = reduce(stopping.state, {
    type: "capture.audio",
    chunk: Uint8Array.from([1, 0, 2, 0]).buffer,
  });
  const append = flushed.effects.find(
    (effect) => effect.domain === "wire" && effect.command === "append",
  );
  assert.ok(append);
  assert.equal(append.takeId, owner.takeId);

  state = reduce(flushed.state, {
    type: "wire.result",
    requestId: append.requestId,
    eventId: "flushed_append",
  }).state;
  const stopped = finishCapture(state, owner.takeId);
  assert.ok(
    stopped.effects.some(
      (effect) => effect.domain === "wire" && effect.command === "commit",
    ),
    "the carried append is followed by commit after capture flushes",
  );
});

test("readiness advances monotonically and stale asynchronous completions are inert", () => {
  let state = reduce(createTakeRegistry(), {
    type: "connection.ready",
    generation: 7,
  }).state;
  const duplicate = reduce(state, {
    type: "connection.ready",
    generation: 7,
  });
  assert.deepEqual(duplicate.state, state);
  assert.deepEqual(duplicate.effects, []);

  state = reduce(state, {
    type: "user.start",
    generation: 7,
    context: context(0),
  }).state;
  const stopping = reduce(state, { type: "user.stop", generation: 7 });
  const owner = stopping.effects.find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(owner);
  state = reduce(stopping.state, {
    type: "connection.lost",
    generation: 7,
  }).state;
  state = reduce(state, {
    type: "connection.ready",
    generation: 8,
  }).state;
  const before = structuredClone(state);

  for (const input of [
    { type: "connection.ready", generation: 7 },
    {
      type: "capture.stopped",
      generation: 7,
      takeId: owner.takeId,
      ok: true,
    },
    {
      type: "wire.result",
      generation: 7,
      requestId: 1,
      eventId: "late_wire",
    },
  ]) {
    const stale = reduce(state, input);
    assert.deepEqual(stale.state, before);
    assert.deepEqual(stale.effects, []);
    state = stale.state;
  }
});
