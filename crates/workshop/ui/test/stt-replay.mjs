// Speech-sandbox replay through the take reducer. The gateway replay test
// owns every section of baseline.json and metrics.json except `ui`; this
// file reads them but rewrites only `ui`, keeping every other byte, so the
// Rust writers' float literals such as `550.0` survive.

import assert from "node:assert/strict";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const replayDir = path.resolve(testDir, "../../../gateway/stt/api/tests/fixtures/replay");
const UPDATE_VARIABLE = "PROMPTFORGE_REPLAY_UPDATE";
const UI_SECTION = "ui";
const SNAPSHOTS_SUFFIX = ".snapshots.json";
const ITEM_ID = "replay";

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export {
        createTakeRegistry,
        reduceTakeRegistry,
      } from "./src/parts/take/take-registry.ts";
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

const { createTakeRegistry, reduceTakeRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

function readJson(file) {
  return JSON.parse(readFileSync(path.join(replayDir, file), "utf8"));
}

function byKey([left], [right]) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function sorted(record) {
  return Object.fromEntries(Object.entries(record).sort(byKey));
}

function rewriteSection(file, value) {
  const target = path.join(replayDir, file);
  const sections = JSON.parse(readFileSync(target, "utf8"), (_key, parsed, context) =>
    typeof parsed === "number" ? JSON.rawJSON(context.source) : parsed,
  );
  const next = sorted({ ...sections, [UI_SECTION]: value });
  writeFileSync(target, `${JSON.stringify(next, null, 2)}\n`);
}

function fixtures() {
  return readdirSync(replayDir)
    .filter((file) => file.endsWith(SNAPSHOTS_SUFFIX))
    .sort()
    .map((file) => [file.slice(0, -SNAPSHOTS_SUFFIX.length), readJson(file)]);
}

function words(text) {
  return text.split(/\s+/).filter((word) => word !== "");
}

function changedWords(earlier, later) {
  const before = words(earlier);
  const after = words(later);
  let common = 0;
  while (common < before.length && before[common] === after[common]) {
    common += 1;
  }
  return before.length - common;
}

function ratio(numerator, denominator) {
  return denominator === 0 ? 0 : numerator / denominator;
}

/** Counts the editor text sequence the way the gateway replay counts transcripts. */
function renderedMetrics(rendered) {
  const counts = rendered
    .slice(1)
    .map((later, index) => changedWords(rendered[index], later));
  const changed = counts.reduce((sum, count) => sum + count, 0);
  const changedPairs = counts.filter((count) => count > 0).length;
  const completedWords = words(rendered.at(-1) ?? "").length;
  return {
    changed_pairs: changedPairs,
    changed_words: changed,
    completed_words: completedWords,
    pairs: counts.length,
    upsr: ratio(changedPairs, counts.length),
    upwr: ratio(changed, completedWords),
  };
}

/** The gateway replay's UPWR rule, compared without division. */
function thresholdViolations(current, baseline) {
  return Object.entries(current).flatMap(([name, metrics]) => {
    const base = baseline[name];
    if (base === undefined) {
      return [`${name} has no ui baseline section`];
    }
    return metrics.changed_words * base.completed_words >
      base.changed_words * metrics.completed_words
      ? [`${name} rendered UPWR ${metrics.upwr} is above its baseline ${base.upwr}`]
      : [];
  });
}

/** Whether total changed over total completed words, across fixtures, is strictly below the baseline's. */
function aggregateUpwrBelowBaseline(current, baseline) {
  const names = Object.keys(current);
  const total = (metrics, field) =>
    names.reduce((sum, name) => sum + (metrics[name]?.[field] ?? 0), 0);
  return (
    total(current, "changed_words") * total(baseline, "completed_words") <
    total(baseline, "changed_words") * total(current, "completed_words")
  );
}

function applyEdits(text, effects) {
  let next = text;
  for (const effect of effects) {
    if (effect.domain !== "editor" || effect.command !== "replace") {
      continue;
    }
    assert.ok(
      effect.from >= 0 && effect.from <= effect.to && effect.to <= next.length,
      `replace [${effect.from}, ${effect.to}] lies inside ${JSON.stringify(next)}`,
    );
    next = next.slice(0, effect.from) + effect.text + next.slice(effect.to);
  }
  return next;
}

function hypothesis(snapshot) {
  return {
    type: "conversation.item.input_audio_transcription.hypothesis",
    event_id: `hypothesis_${snapshot.revision}`,
    item_id: ITEM_ID,
    content_index: 0,
    revision: snapshot.revision,
    transcript: snapshot.transcript,
    finalized: snapshot.finalized,
    agreed: snapshot.agreed,
    tentative: snapshot.tentative,
    audio_start_ms: snapshot.audio_start_ms,
    audio_end_ms: snapshot.audio_end_ms,
  };
}

/** The editor text after each snapshot, then after the completed transcript lands. */
function render({ snapshots, completed }) {
  let state = createTakeRegistry();
  let editor = "";
  const step = (input) => {
    const result = reduceTakeRegistry(state, input);
    state = result.state;
    editor = applyEdits(editor, result.effects);
    return result.effects;
  };
  step({
    type: "user.start",
    context: { range: { start: 0, end: 0 }, original: "", compositionPrefix: "" },
  });
  const rendered = snapshots.map((snapshot) => {
    step({ type: "server.event", event: hypothesis(snapshot) });
    return editor;
  });
  const stop = step({ type: "user.stop" }).find(
    (effect) => effect.domain === "capture" && effect.command === "stop",
  );
  assert.ok(stop, "stopping the take requests a capture stop");
  const commit = step({ type: "capture.stopped", takeId: stop.takeId, ok: true }).find(
    (effect) => effect.domain === "wire" && effect.command === "commit",
  );
  assert.ok(commit, "a finished capture commits the take");
  step({ type: "wire.result", requestId: commit.requestId, eventId: "commit_replay" });
  step({
    type: "server.event",
    event: {
      type: "input_audio_buffer.committed",
      event_id: "committed_replay",
      item_id: ITEM_ID,
      previous_item_id: null,
    },
  });
  step({
    type: "server.event",
    event: {
      type: "conversation.item.input_audio_transcription.completed",
      event_id: "completed_replay",
      item_id: ITEM_ID,
      content_index: 0,
      transcript: completed,
      usage: { type: "duration", seconds: 0 },
    },
  });
  assert.equal(state.takes.length, 0, "the completed transcript retires the take");
  rendered.push(editor);
  return rendered;
}

test("every replay fixture renders through the take reducer within its ui baseline", () => {
  const update = process.env[UPDATE_VARIABLE] === "1";
  const loaded = fixtures();
  assert.ok(
    loaded.filter(([name]) => name.startsWith("scripted-")).length >= 4,
    "every scripted fixture is discovered",
  );
  assert.ok(
    loaded.some(([name]) => name === "jfk-native"),
    "the native jfk fixture is discovered",
  );

  const current = {};
  for (const [name, outcome] of loaded) {
    const rendered = render(outcome);
    assert.equal(
      rendered.at(-1),
      outcome.completed.trimEnd(),
      `${name} lands its completed transcript`,
    );
    current[name] = renderedMetrics(rendered);
  }

  if (update) {
    rewriteSection("metrics.json", current);
  } else {
    assert.deepEqual(
      readJson("metrics.json")[UI_SECTION],
      current,
      `metrics.json ui section drifted; run with ${UPDATE_VARIABLE}=1 to regenerate it`,
    );
  }

  const recorded = readJson("baseline.json")[UI_SECTION] ?? {};
  const unrecorded = Object.keys(current).filter((name) => !(name in recorded));
  const baseline = { ...recorded };
  if (update && unrecorded.length > 0) {
    for (const name of unrecorded) {
      baseline[name] = current[name];
    }
    rewriteSection("baseline.json", sorted(baseline));
  }
  const violations = thresholdViolations(current, baseline);
  assert.deepEqual(violations, [], `rendered-text thresholds are broken:\n${violations.join("\n")}`);
  assert.ok(
    aggregateUpwrBelowBaseline(current, baseline),
    "aggregate rendered UPWR across fixtures is below the ui baseline aggregate",
  );
});

test("a final that lands in silence shows the last word and its period before the next word", () => {
  const outcome = readJson("scripted-silence-short-word.snapshots.json");
  const rendered = render(outcome);
  const beforeNextWord = outcome.snapshots.findIndex((snapshot) =>
    snapshot.transcript.includes("Hey"),
  ) - 1;
  assert.ok(beforeNextWord >= 0, "the fixture says a word after its silence");
  assert.equal(rendered[beforeNextWord], "About one tick faster than the plan's 0.6 seconds.");
});

test("rendered UPWR and UPSR count earlier editor words after the exact common prefix", () => {
  assert.deepEqual(
    renderedMetrics(["ask not", "Ask not what", "Ask not, what your", "Ask not, what your country"]),
    {
      changed_pairs: 2,
      changed_words: 4,
      completed_words: 5,
      pairs: 3,
      upsr: 2 / 3,
      upwr: 4 / 5,
    },
  );
});

test("the ui threshold flags rendered UPWR above baseline and a fixture without one", () => {
  const counted = (changed, completed) => ({
    changed_words: changed,
    completed_words: completed,
    upwr: changed / completed,
  });
  const baseline = { "scripted-a": counted(2, 10) };
  const check = (metrics, name = "scripted-a") =>
    thresholdViolations({ [name]: metrics }, baseline).length;

  assert.equal(check(counted(2, 10)), 0);
  assert.equal(check(counted(1, 10)), 0);
  assert.equal(check(counted(4, 20)), 0, "an equal ratio over more words passes");
  assert.equal(check(counted(3, 10)), 1, "UPWR rises");
  assert.equal(check(counted(2, 10), "scripted-b"), 1, "a fixture without a baseline section fails");
});

test("the aggregate ui rule sums every fixture and requires strictly lower UPWR", () => {
  const counted = (changed, completed) => ({ changed_words: changed, completed_words: completed });
  const baseline = { a: counted(4, 10), b: counted(2, 10) };

  assert.equal(aggregateUpwrBelowBaseline({ a: counted(3, 10), b: counted(2, 10) }, baseline), true);
  assert.equal(
    aggregateUpwrBelowBaseline({ a: counted(6, 10), b: counted(0, 10) }, baseline),
    false,
    "an equal aggregate is not below",
  );
  assert.equal(
    aggregateUpwrBelowBaseline({ a: counted(1, 10), b: counted(6, 10) }, baseline),
    false,
    "one fixture's rise outweighs another's fall",
  );
});
