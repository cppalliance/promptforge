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

// Agreed text renders whole, so no word is held back as tentative.
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

function replacements(effects) {
  return effects.filter(
    (effect) => effect.domain === "editor" && effect.command === "replace",
  );
}

function announcements(effects) {
  return effects
    .filter((effect) => effect.domain === "status" && effect.command === "announce")
    .map(({ text }) => text);
}

// A target's undo history: a transient replace stays out of it, and any
// other replace is one undo step back to the text before it.
function applyWithHistory(target, effects) {
  let { text } = target;
  const undo = [...target.undo];
  for (const effect of replacements(effects)) {
    assert.ok(effect.from >= 0 && effect.from <= effect.to && effect.to <= text.length);
    if (!effect.transient) {
      undo.push(text);
    }
    text = text.slice(0, effect.from) + effect.text + text.slice(effect.to);
  }
  return { text, undo };
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
      transient: true,
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
      transient: true,
    },
  ]);
});

test("interim writes stay out of undo history and a completion lands as one undo step", () => {
  let target = { text: "Note: xy!", undo: [] };
  let state = start(createTakeRegistry(), context(6, 8, "xy")).state;
  const step = (result) => {
    state = result.state;
    target = applyWithHistory(target, result.effects);
    return replacements(result.effects);
  };

  const interim = step(server(state, hypothesis("item", "ask", 1)));
  interim.push(...step(server(state, hypothesis("item", "ask not", 2))));
  assert.ok(interim.length > 0);
  assert.ok(interim.every((effect) => effect.transient), "every interim write is transient");
  assert.deepEqual(target, { text: "Note: ask not!", undo: [] });

  state = stopAndCommit(state, "client_item").state;
  state = server(state, committed("item")).state;
  assert.deepEqual(step(server(state, completion("item", "ask not what"))), [
    { domain: "editor", command: "replace", from: 6, to: 13, text: "xy", transient: true },
    { domain: "editor", command: "replace", from: 6, to: 8, text: "ask not what", transient: false },
  ]);
  assert.deepEqual(
    target,
    { text: "Note: ask not what!", undo: ["Note: xy!"] },
    "the one undo step restores the text from before the dictation",
  );
});

test("a take hands the target's captured content back before its text lands, and spans the captured width after", () => {
  const content = { captured: "selection" };
  const captured = { ...context(4, 9, "xy"), content };
  const edits = (effects) =>
    effects.filter(
      (effect) =>
        effect.domain === "editor" && (effect.command === "replace" || effect.command === "restore"),
    );

  let state = start(createTakeRegistry(), captured).state;
  state = server(state, hypothesis("item", "spoken")).state;
  state = stopAndCommit(state, "client_item").state;
  state = server(state, committed("item")).state;
  assert.deepEqual(edits(server(state, completion("item", "final")).effects), [
    { domain: "editor", command: "restore", from: 4, to: 10, content },
    { domain: "editor", command: "replace", from: 4, to: 9, text: "final", transient: false },
  ]);

  let result = start(createTakeRegistry(), captured);
  result = server(result.state, parts("item", 1, { agreed: "kept", tentative: " dropped words" }));
  assert.deepEqual(edits(reduce(result.state, { type: "connection.lost" }).effects), [
    { domain: "editor", command: "restore", from: 4, to: 16, content },
    { domain: "editor", command: "replace", from: 4, to: 9, text: "kept", transient: false },
  ]);

  state = start(createTakeRegistry(), captured).state;
  assert.deepEqual(
    edits(reduce(state, { type: "user.discard" }).effects),
    [],
    "an unwritten take leaves its selection untouched",
  );
  state = server(state, hypothesis("item", "spoken")).state;
  assert.deepEqual(edits(reduce(state, { type: "user.discard" }).effects), [
    { domain: "editor", command: "restore", from: 4, to: 10, content },
  ]);
});

test("a socket loss lands finalized and agreed text, drops the tentative tail, and releases the take", () => {
  let target = { text: "Note", undo: [] };
  let result = start(createTakeRegistry(), context(4, 4, "", " "));
  result = server(
    result.state,
    parts("item", 1, { finalized: "Ask not ", agreed: "what your", tentative: " country can" }),
  );
  target = applyWithHistory(target, result.effects);
  assert.equal(target.text, "Note Ask not what your country");

  result = reduce(result.state, { type: "connection.lost" });
  target = applyWithHistory(target, result.effects);
  assert.deepEqual(
    target,
    { text: "Note Ask not what your", undo: ["Note"] },
    "the kept text lands as one undo step",
  );
  assert.deepEqual(result.state.takes, []);
  assert.equal(result.state.activeTakeId, null);
  assert.ok(
    result.effects.some(
      (effect) => effect.domain === "editor" && effect.command === "read-only" && !effect.readOnly,
    ),
    "releasing the take unlocks the editor",
  );
  assert.ok(
    result.effects.some((effect) => effect.domain === "capture" && effect.command === "stop"),
  );
});

test("a socket loss with only tentative words restores the take's original text", () => {
  let target = { text: "Hey xy!", undo: [] };
  let result = start(createTakeRegistry(), context(4, 6, "xy"));
  result = server(result.state, parts("item", 1, { tentative: "spoken words" }));
  target = applyWithHistory(target, result.effects);
  assert.equal(target.text, "Hey spoken!");

  result = reduce(result.state, { type: "connection.lost" });
  target = applyWithHistory(target, result.effects);
  assert.deepEqual(target, { text: "Hey xy!", undo: [] });
  assert.deepEqual(result.state.takes, []);
});

test("the live region hears each finished stable sentence once and a completed take's rest", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  const heard = (event) => {
    const result = server(state, event);
    state = result.state;
    return announcements(result.effects);
  };

  assert.deepEqual(
    heard(parts("item", 1, { agreed: "Ask not", tentative: " what." })),
    [],
    "a tentative sentence end is not announced",
  );
  assert.deepEqual(heard(parts("item", 2, { finalized: "Ask not what.", tentative: " Ask" })), [
    "Ask not what.",
  ]);
  assert.deepEqual(
    heard(parts("item", 3, { finalized: "Ask not what.", agreed: " Ask 3.5 times", tentative: " now" })),
    [],
    "an announced sentence is not repeated and a period inside a number ends nothing",
  );
  assert.deepEqual(
    heard(parts("item", 4, { finalized: "Ask not what.", agreed: " Ask 3.5 times? Then go! And" })),
    ["Ask 3.5 times? Then go!"],
  );

  state = stopAndCommit(state, "client_item").state;
  state = server(state, committed("item")).state;
  assert.deepEqual(
    heard(completion("item", "Ask not what. Ask 3.5 times? Then go! And stay")),
    ["And stay"],
  );
});

test("a completion announces nothing once every sentence was heard", () => {
  let state = start(createTakeRegistry(), context(0)).state;
  let result = server(state, hypothesis("item", "Hello there."));
  assert.deepEqual(announcements(result.effects), ["Hello there."]);
  state = stopAndCommit(result.state, "client_item").state;
  state = server(state, committed("item")).state;
  result = server(state, completion("item", "Hello there.  "));
  assert.deepEqual(announcements(result.effects), []);

  state = start(result.state, context(12, 12, "", " ")).state;
  state = stopAndCommit(state, "client_unheard").state;
  state = server(state, committed("unheard")).state;
  result = server(state, completion("unheard", "Never heard. At all"));
  assert.deepEqual(announcements(result.effects), ["Never heard. At all"]);
});

test("a hypothesis patches only its changed words and an unchanged one writes nothing", () => {
  let state = start(createTakeRegistry(), context(0)).state;

  let result = server(state, hypothesis("whole", "ask not", 1));
  assert.deepEqual(replacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 0,
      to: 0,
      text: "ask not",
      transient: true,
    },
  ]);
  state = result.state;

  result = server(state, hypothesis("whole", "ask not what", 2));
  assert.deepEqual(replacements(result.effects), [
    {
      domain: "editor",
      command: "replace",
      from: 7,
      to: 7,
      text: " what",
      transient: true,
    },
  ]);
  state = result.state;

  result = server(state, hypothesis("whole", "ask not what", 3));
  assert.deepEqual(replacements(result.effects), []);
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
    { domain: "editor", command: "replace", from: 0, to: 11, text: "", transient: true },
    { domain: "editor", command: "replace", from: 0, to: 0, text: "complete", transient: false },
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
    "the append is followed by commit after capture flushes",
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
