// Scores a native Realtime capture against a line file. The capture is the
// JSON the gateway's native Realtime capture test writes; its server events
// render through the take reducer the way Workshop shows them. A line file
// is an array of `{ text, start_ms, end_ms }`, one entry per scripted line,
// with times in milliseconds of clip audio. The capture appends audio at
// real-time pace from its first append, so clip time and the capture's
// receive times share one clock, to within one append.
//
// Per line, the score reports the final latency: the receive time of the
// first final pass after which the line reads exactly as it does in the
// completed transcript, minus the line's speech end. A final pass is a
// hypothesis whose finalized text changes, read with its agreed text, or the
// completed transcript itself; agreed text alone is not final until a final
// pass lands. The score also reports whether the line's final text keeps the
// script's terminal punctuation, each distinct run of words that renders
// during the following pause beyond the script spoken so far, and, for the
// take, the completed transcript and its word errors against the whole
// script.
// Words compare lowercased, with leading and trailing punctuation removed;
// line texts compare exactly.
//
// With PROMPTFORGE_REALTIME_CAPTURE naming a capture,
// PROMPTFORGE_CAPTURE_LINES a line file, and PROMPTFORGE_CAPTURE_SCORE an
// output path, the capture is scored into the output; otherwise only the
// synthetic tests run.

import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import test from "node:test";
import { applyEdits, createTakeRegistry, reduceTakeRegistry } from "./take-reducer-loader.mjs";

const CAPTURE_VARIABLE = "PROMPTFORGE_REALTIME_CAPTURE";
const LINES_VARIABLE = "PROMPTFORGE_CAPTURE_LINES";
const SCORE_VARIABLE = "PROMPTFORGE_CAPTURE_SCORE";
const HYPOTHESIS = "conversation.item.input_audio_transcription.hypothesis";
const COMMITTED = "input_audio_buffer.committed";
const COMPLETED = "conversation.item.input_audio_transcription.completed";

/**
 * Plays the capture's server events through the take reducer, stopping the
 * take where the capture committed. Returns the editor text after each
 * event, the stable text of each final pass, and the editor text once the
 * take completes.
 */
function render(capture) {
  let state = createTakeRegistry();
  let editor = "";
  const step = (input) => {
    const result = reduceTakeRegistry(state, input);
    state = result.state;
    editor = applyEdits(editor, result.effects);
    return result.effects;
  };
  const stop = () => {
    const stopped = step({ type: "user.stop" }).find(
      (effect) => effect.domain === "capture" && effect.command === "stop",
    );
    assert.ok(stopped, "stopping the take requests a capture stop");
    const commit = step({ type: "capture.stopped", takeId: stopped.takeId, ok: true }).find(
      (effect) => effect.domain === "wire" && effect.command === "commit",
    );
    assert.ok(commit, "a finished capture commits the take");
    step({ type: "wire.result", requestId: commit.requestId, eventId: "commit" });
  };

  step({
    type: "user.start",
    context: { range: { start: 0, end: 0 }, original: "", compositionPrefix: "" },
  });
  const renders = [];
  const finals = [];
  let committed = false;
  let revision = -1;
  let finalized = "";
  let completedText = null;
  for (const { at_ms: atMs, event } of capture.events) {
    if (!committed && atMs >= capture.commit_ms) {
      committed = true;
      stop();
    }
    step({ type: "server.event", event });
    renders.push({ at_ms: atMs, text: editor });
    if (event.type === HYPOTHESIS && event.revision > revision) {
      revision = event.revision;
      if (event.finalized !== finalized) {
        finalized = event.finalized;
        finals.push({ at_ms: atMs, text: event.finalized + event.agreed });
      }
    } else if (event.type === COMPLETED) {
      finals.push({ at_ms: atMs, text: event.transcript });
      completedText = editor;
    }
  }
  return { renders, finals, completed: completedText };
}

/** Each whitespace-separated token with its compared form; tokens of only punctuation drop out. */
function words(text) {
  return text.split(/\s+/u).flatMap((token) => {
    const word = token
      .toLowerCase()
      .replaceAll("\u2019", "'")
      .replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, "");
    return word === "" ? [] : [{ token, word }];
  });
}

function compared(text) {
  return words(text).map(({ word }) => word);
}

/**
 * A minimum-edit alignment of hypothesis words to reference words, with
 * the reference index each hypothesis word matches or substitutes, or
 * null for an insertion. With `prefix`, the hypothesis aligns to the
 * reference prefix that costs least, the longest on a tie. Ties align
 * trailing hypothesis words as insertions.
 */
function align(hypothesis, reference, prefix = false) {
  const rows = hypothesis.length + 1;
  const columns = reference.length + 1;
  const cost = Array.from({ length: rows }, (_, i) =>
    Array.from({ length: columns }, (_, j) => (i === 0 ? j : j === 0 ? i : 0)),
  );
  const differs = (i, j) => (hypothesis[i - 1] === reference[j - 1] ? 0 : 1);
  for (let i = 1; i < rows; i += 1) {
    for (let j = 1; j < columns; j += 1) {
      cost[i][j] = Math.min(
        cost[i - 1][j - 1] + differs(i, j),
        cost[i - 1][j] + 1,
        cost[i][j - 1] + 1,
      );
    }
  }
  const last = cost[rows - 1];
  const end = prefix
    ? last.reduce((best, value, j) => (value <= last[best] ? j : best), 0)
    : columns - 1;
  const referenceIndex = Array(hypothesis.length).fill(null);
  const counts = { substitutions: 0, deletions: 0, insertions: 0 };
  let i = rows - 1;
  let j = end;
  while (i > 0 || j > 0) {
    if (i > 0 && cost[i][j] === cost[i - 1][j] + 1) {
      counts.insertions += 1;
      i -= 1;
    } else if (i > 0 && j > 0 && cost[i][j] === cost[i - 1][j - 1] + differs(i, j)) {
      referenceIndex[i - 1] = j - 1;
      counts.substitutions += differs(i, j);
      i -= 1;
      j -= 1;
    } else {
      counts.deletions += 1;
      j -= 1;
    }
  }
  return { referenceIndex, ...counts };
}

/** The sentence or clause mark that ends a text, past any closing quotes or brackets. */
function terminalMark(text) {
  return /([.?!,;:])[\p{Pe}\p{Pf}"']*\s*$/u.exec(text)?.[1] ?? null;
}

/** Whether `text` ends with the script line's mark, or null when the script line has none. */
function keepsPunctuation(text, script) {
  const mark = terminalMark(script);
  return mark === null ? null : terminalMark(text) === mark;
}

/**
 * Each line's text within a transcript: its tokens from the first aligned
 * to one of the line's script words through the last, with any inserted
 * tokens between them, or null when no token aligns to the line.
 */
function lineTexts(text, scriptWords, lineEnds) {
  const tokens = words(text);
  const { referenceIndex } = align(
    tokens.map(({ word }) => word),
    scriptWords,
    true,
  );
  return lineEnds.map((end, index) => {
    const start = lineEnds[index - 1] ?? 0;
    const inside = (reference) => reference !== null && reference >= start && reference < end;
    const first = referenceIndex.findIndex(inside);
    if (first < 0) {
      return null;
    }
    return tokens
      .slice(first, referenceIndex.findLastIndex(inside) + 1)
      .map(({ token }) => token)
      .join(" ");
  });
}

/** The shown words after the last one that aligns to a spoken script word. */
function unsaidWords(text, spoken) {
  const shown = compared(text);
  const { referenceIndex } = align(shown, spoken, true);
  return shown.slice(referenceIndex.findLastIndex((index) => index !== null) + 1);
}

/** Each nonempty run once, in the order first seen. */
function distinctRuns(runs) {
  return [
    ...new Map(runs.filter((run) => run.length > 0).map((run) => [run.join(" "), run])).values(),
  ];
}

function wordErrors(text, scriptWords) {
  const { substitutions, deletions, insertions } = align(compared(text), scriptWords);
  const errors = substitutions + deletions + insertions;
  return {
    substitutions,
    deletions,
    insertions,
    errors,
    script_words: scriptWords.length,
    rate: errors / scriptWords.length,
  };
}

function scoreCapture(capture, lines) {
  const { renders, finals, completed } = render(capture);
  const script = lines.map(({ text }) => compared(text));
  const scriptWords = script.flat();
  let through = 0;
  const lineEnds = script.map((lineWords) => (through += lineWords.length));
  const completedLines =
    completed === null ? [] : lineTexts(completed, scriptWords, lineEnds);
  const finalLines = finals.map(({ at_ms: atMs, text }) => ({
    at_ms: atMs,
    lines: lineTexts(text, scriptWords, lineEnds),
  }));
  return {
    lines: lines.map((line, index) => {
      const finalText = completedLines[index] ?? null;
      const final =
        finalText === null
          ? undefined
          : finalLines[
              finalLines.findLastIndex((candidate) => candidate.lines[index] !== finalText) + 1
            ];
      const pauseEnd = lines[index + 1]?.start_ms ?? capture.commit_ms;
      const spoken = scriptWords.slice(0, lineEnds[index]);
      return {
        text: line.text,
        start_ms: line.start_ms,
        end_ms: line.end_ms,
        final_text: finalText,
        final_ms: final?.at_ms ?? null,
        final_latency_ms: final === undefined ? null : final.at_ms - line.end_ms,
        punctuation_kept: finalText === null ? null : keepsPunctuation(finalText, line.text),
        pause_unsaid_runs: distinctRuns(
          renders
            .filter(({ at_ms: atMs }) => atMs >= line.end_ms && atMs < pauseEnd)
            .map(({ text }) => unsaidWords(text, spoken)),
        ),
      };
    }),
    take: { completed, word_errors: wordErrors(completed ?? "", scriptWords) },
  };
}

function readLines(file) {
  const lines = JSON.parse(readFileSync(file, "utf8"));
  assert.ok(Array.isArray(lines) && lines.length > 0, `${file} lists the script's lines`);
  lines.forEach((line, index) => {
    assert.ok(
      typeof line.text === "string" && compared(line.text).length > 0,
      `line ${index} of ${file} has script words`,
    );
    assert.ok(
      Number.isFinite(line.start_ms) && Number.isFinite(line.end_ms) && line.start_ms < line.end_ms,
      `line ${index} of ${file} starts before it ends`,
    );
    assert.ok(
      index === 0 || lines[index - 1].end_ms <= line.start_ms,
      `line ${index} of ${file} starts after the line before it ends`,
    );
  });
  return lines;
}

const scoring = [CAPTURE_VARIABLE, LINES_VARIABLE, SCORE_VARIABLE].map((name) => process.env[name]);

test(
  "the capture the environment names is scored into its output",
  {
    skip:
      scoring.some((value) => !value) &&
      `set ${CAPTURE_VARIABLE}, ${LINES_VARIABLE}, and ${SCORE_VARIABLE} to score a capture`,
  },
  () => {
    const [capturePath, linesPath, scorePath] = scoring;
    const score = scoreCapture(JSON.parse(readFileSync(capturePath, "utf8")), readLines(linesPath));
    assert.notEqual(score.take.completed, null, `${capturePath} completes its take`);
    writeFileSync(scorePath, `${JSON.stringify(score, null, 2)}\n`);
  },
);

const ITEM_ID = "item_capture";

function hypothesis(atMs, revision, finalized, agreed, tentative) {
  return {
    at_ms: atMs,
    event: {
      type: HYPOTHESIS,
      event_id: `hypothesis_${revision}`,
      item_id: ITEM_ID,
      content_index: 0,
      revision,
      transcript: finalized + agreed + tentative,
      finalized,
      agreed,
      tentative,
      audio_start_ms: 0,
      audio_end_ms: atMs,
    },
  };
}

function committed(atMs) {
  return {
    at_ms: atMs,
    event: { type: COMMITTED, event_id: "committed", item_id: ITEM_ID, previous_item_id: null },
  };
}

function completed(atMs, transcript) {
  return {
    at_ms: atMs,
    event: {
      type: COMPLETED,
      event_id: "completed",
      item_id: ITEM_ID,
      content_index: 0,
      transcript,
      usage: { type: "duration", seconds: 0 },
    },
  };
}

const LINE_ONE = "Ask not what your country can do for you.";
const LINE_TWO = "Ask what you can do for your country.";

/** Two lines; the first final lands in the pause, the second after the commit without its period. */
const SYNTHETIC_LINES = [
  { text: LINE_ONE, start_ms: 0, end_ms: 2_000 },
  { text: LINE_TWO, start_ms: 3_000, end_ms: 5_000 },
];

const SYNTHETIC_CAPTURE = {
  audio_ms: 5_500,
  chunk_ms: 100,
  last_append_ms: 5_400,
  commit_ms: 5_600,
  completed: `${LINE_ONE} Ask what you can do for my country`,
  events: [
    hypothesis(1_200, 1, "", "", "Ask not what your"),
    hypothesis(2_100, 2, "", "Ask not what your country", " can do for you. Ask"),
    hypothesis(2_400, 3, LINE_ONE, "", ""),
    hypothesis(2_600, 4, LINE_ONE, "", " Thank you so"),
    hypothesis(2_800, 5, LINE_ONE, "", ""),
    hypothesis(4_200, 6, LINE_ONE, " Ask what you", " can do for"),
    hypothesis(5_300, 7, LINE_ONE, ` ${LINE_TWO}`, ""),
    committed(5_700),
    hypothesis(5_900, 8, `${LINE_ONE} Ask what you can do for my country`, "", ""),
    completed(6_000, `${LINE_ONE} Ask what you can do for my country`),
  ],
};

test("a line's final latency runs from its speech end to the final pass that lands its text", () => {
  const score = scoreCapture(SYNTHETIC_CAPTURE, SYNTHETIC_LINES);
  assert.deepEqual(
    score.lines.map((line) => line.final_latency_ms),
    [400, 900],
  );
});

test("a final keeps or loses its line's terminal punctuation", () => {
  const score = scoreCapture(SYNTHETIC_CAPTURE, SYNTHETIC_LINES);
  assert.deepEqual(
    score.lines.map((line) => line.punctuation_kept),
    [true, false],
  );
});

test("a pause counts rendered words beyond the script spoken so far, never said words", () => {
  const score = scoreCapture(SYNTHETIC_CAPTURE, SYNTHETIC_LINES);
  assert.deepEqual(
    score.lines.map((line) => line.pause_unsaid_runs),
    [[["thank", "you"]], []],
  );
});

test("a pause reports each distinct unsaid run it shows, not only the longest", () => {
  const first = "Can you check it?";
  const second = "Use it now.";
  const lines = [
    { text: first, start_ms: 0, end_ms: 1_000 },
    { text: second, start_ms: 3_000, end_ms: 4_000 },
  ];
  const transcript = `${first} ${second}`;
  const capture = {
    audio_ms: 4_500,
    chunk_ms: 100,
    last_append_ms: 4_400,
    commit_ms: 4_500,
    completed: transcript,
    events: [
      hypothesis(1_200, 1, first, "", ""),
      hypothesis(1_500, 2, first, "", " Thank you so"),
      hypothesis(1_700, 3, first, "", " Thank you so"),
      hypothesis(1_900, 4, first, "", ""),
      hypothesis(2_200, 5, first, "", " Bye now so"),
      hypothesis(2_500, 6, first, "", ""),
      hypothesis(4_300, 7, transcript, "", ""),
      committed(4_600),
      completed(4_700, transcript),
    ],
  };
  assert.deepEqual(
    scoreCapture(capture, lines).lines.map((line) => line.pause_unsaid_runs),
    [[["thank", "you"], ["bye", "now"]], []],
  );
});

test("the completed transcript reports its word errors against the whole script", () => {
  const score = scoreCapture(SYNTHETIC_CAPTURE, SYNTHETIC_LINES);
  assert.equal(score.take.completed, `${LINE_ONE} Ask what you can do for my country`);
  assert.deepEqual(score.take.word_errors, {
    substitutions: 1,
    deletions: 0,
    insertions: 0,
    errors: 1,
    script_words: 17,
    rate: 1 / 17,
  });
});

test("word errors count a dropped script word, an inserted word, and a substitution apart", () => {
  const transcript = "Ask not what country can do for you. Ask what you can do for my own country";
  const capture = {
    audio_ms: 5_500,
    chunk_ms: 100,
    last_append_ms: 5_400,
    commit_ms: 5_600,
    completed: transcript,
    events: [committed(5_700), completed(5_800, transcript)],
  };
  const score = scoreCapture(capture, SYNTHETIC_LINES);
  assert.equal(score.take.completed, transcript);
  assert.deepEqual(score.take.word_errors, {
    substitutions: 1,
    deletions: 1,
    insertions: 1,
    errors: 3,
    script_words: 17,
    rate: 3 / 17,
  });
});

test("a line's final is the first final pass after which it reads as the completed transcript does", () => {
  const first = "Can you check it?";
  const second = "Use PromptForge now.";
  const third = "Keep talking, so the window cuts here.";
  const lines = [
    { text: first, start_ms: 0, end_ms: 1_000 },
    { text: second, start_ms: 2_000, end_ms: 3_000 },
    { text: third, start_ms: 4_000, end_ms: 6_000 },
  ];
  const transcript = `${first} ${second} ${third}`;
  const capture = {
    audio_ms: 6_500,
    chunk_ms: 100,
    last_append_ms: 6_400,
    commit_ms: 7_000,
    completed: transcript,
    events: [
      hypothesis(1_300, 1, "", first, ""),
      hypothesis(1_700, 2, first, "", ""),
      hypothesis(2_600, 3, first, " Use prompt forge now.", ""),
      hypothesis(3_400, 4, `${first} Use`, " prompt forge now.", ""),
      hypothesis(3_600, 5, `${first} ${second}`, "", ""),
      hypothesis(6_800, 6, `${first} ${second} Keep talking,`, " so the window cuts here.", ""),
      committed(7_100),
      completed(7_200, transcript),
    ],
  };
  assert.deepEqual(
    scoreCapture(capture, lines).lines.map((line) => line.final_latency_ms),
    [700, 600, 800],
    "agreed text waits for a final pass, a final pass with other text does not count, and agreed text a final pass leaves behind does",
  );
});

test("a final pass that revises a line's matching text moves its final to the pass that restores it", () => {
  const first = "Can you check it?";
  const second = "Use it now.";
  const lines = [
    { text: first, start_ms: 0, end_ms: 1_000 },
    { text: second, start_ms: 2_000, end_ms: 3_000 },
  ];
  const transcript = `${first} ${second}`;
  const capture = {
    audio_ms: 3_500,
    chunk_ms: 100,
    last_append_ms: 3_400,
    commit_ms: 4_000,
    completed: transcript,
    events: [
      hypothesis(1_300, 1, "Can", " you check it?", ""),
      hypothesis(1_600, 2, "Can you", " chuck it?", ""),
      hypothesis(1_900, 3, first, "", ""),
      hypothesis(3_400, 4, transcript, "", ""),
      committed(4_100),
      completed(4_200, transcript),
    ],
  };
  assert.deepEqual(
    scoreCapture(capture, lines).lines.map((line) => line.final_latency_ms),
    [900, 400],
  );
});

test("a split or misheard word still completes its line and never counts as unsaid", () => {
  const lines = [
    { text: "I cannot wait.", start_ms: 0, end_ms: 1_000 },
    { text: "Let us go.", start_ms: 2_000, end_ms: 3_000 },
  ];
  const capture = {
    audio_ms: 3_100,
    chunk_ms: 100,
    last_append_ms: 3_000,
    commit_ms: 3_200,
    completed: "I can not wait. Let us goal.",
    events: [
      hypothesis(1_300, 1, "I can not wait.", "", ""),
      hypothesis(1_500, 2, "I can not wait.", "", ""),
      committed(3_300),
      completed(3_500, "I can not wait. Let us goal."),
    ],
  };
  const score = scoreCapture(capture, lines);
  assert.deepEqual(
    score.lines.map(({ final_latency_ms, punctuation_kept, pause_unsaid_runs }) => ({
      final_latency_ms,
      punctuation_kept,
      pause_unsaid_runs,
    })),
    [
      { final_latency_ms: 300, punctuation_kept: true, pause_unsaid_runs: [] },
      { final_latency_ms: 500, punctuation_kept: true, pause_unsaid_runs: [] },
    ],
  );
});
