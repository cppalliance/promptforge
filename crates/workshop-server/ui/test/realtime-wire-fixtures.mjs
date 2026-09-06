import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const fixtureDir = path.join(
  testDir,
  "..",
  "..",
  "..",
  "gateway-stt",
  "tests",
  "fixtures",
  "realtime",
);

const fixtureFiles = [
  "client-events.json",
  "effective-sessions.json",
  "invalid-sequences.json",
  "server-events.json",
  "valid-sequences.json",
];

const clientCases = [
  "input_audio_buffer_append",
  "input_audio_buffer_clear",
  "input_audio_buffer_commit",
  "session_update",
];

const serverCases = [
  "conversation_item_created",
  "error_correlated",
  "error_minimal",
  "error_uncorrelated",
  "input_audio_buffer_cleared",
  "input_audio_buffer_committed",
  "session_created",
  "session_updated",
  "transcription_completed",
  "transcription_delta",
  "transcription_failed",
  "transcription_hypothesis",
];

const validSequenceCases = [
  "clear_retires_only_uncommitted_input",
  "configuration_snapshot_isolation",
  "durable_lineage",
  "engine_replacement",
  "first_event_readiness",
  "hypothesis_negotiation",
  "immediate_commit_and_provisional_promotion",
  "optional_client_ids_and_error_correlation",
  "overlapping_items_reverse_completion",
  "pending_precommit_failure_clear",
  "pending_precommit_failure_commit",
  "saturated_commit_retry",
  "segment_admission_failure",
  "standard_delta_after_item_creation",
];

const invalidSequenceCases = [
  "append_after_precommit_failure",
  "append_invalid_base64",
  "append_limit_exceeded",
  "append_unknown_field",
  "clear_unknown_field",
  "commit_short_audio",
  "commit_unknown_field",
  "dangling_pcm_byte_on_commit",
  "excessive_queue_lag",
  "invalid_client_event_id",
  "invalid_include_type",
  "invalid_prompt_type",
  "malformed_json",
  "maximum_committed_items",
  "maximum_unfinalized_audio",
  "missing_append_audio",
  "missing_client_event_type",
  "missing_session",
  "missing_session_type",
  "non_null_noise_reduction",
  "non_null_turn_detection",
  "result_queue_overload",
  "session_audio_unknown_field",
  "session_input_unknown_field",
  "session_transcription_unknown_field",
  "session_unknown_field",
  "session_update_unknown_field",
  "unknown_event_type",
  "unknown_include",
  "unsupported_delay",
  "unsupported_format_rate",
  "unsupported_format_type",
  "unsupported_keywords",
  "unsupported_language",
  "unsupported_logprobs",
  "unsupported_model",
  "wrong_session_type",
];

const minimumCommitAudioBytes = (24_000 * 2) / 10;

async function fixture(name) {
  const parsed = JSON.parse(await readFile(path.join(fixtureDir, name), "utf8"));
  assert.deepEqual(JSON.parse(JSON.stringify(parsed)), parsed, `${name} round-trips`);
  return parsed;
}

function sortedKeys(value) {
  return Object.keys(value).sort();
}

function assertExactKeys(value, expected, context) {
  assert.deepEqual(sortedKeys(value), [...expected].sort(), `${context} strict keys`);
}

function assertNonemptyString(value, context) {
  assert.equal(typeof value, "string", `${context} is a string`);
  assert.notEqual(value.length, 0, `${context} is nonempty`);
}

function assertSession(session, context) {
  assertExactKeys(session, ["audio", "id", "include", "object", "type"], context);
  assertNonemptyString(session.id, `${context}.id`);
  assert.equal(session.object, "realtime.transcription_session");
  assert.equal(session.type, "transcription");
  assert.ok(Array.isArray(session.include) && session.include.length <= 1);
  if (session.include.length === 1) {
    assert.equal(session.include[0], "item.input_audio_transcription.hypothesis");
  }
  assertExactKeys(session.audio, ["input"], `${context}.audio`);
  const input = session.audio.input;
  assertExactKeys(
    input,
    ["format", "noise_reduction", "transcription", "turn_detection"],
    `${context}.audio.input`,
  );
  assert.equal(input.noise_reduction, null);
  assert.equal(input.turn_detection, null);
  assert.deepEqual(input.format, { type: "audio/pcm", rate: 24000 });
  assertExactKeys(input.transcription, ["model", "prompt"], `${context}.transcription`);
  assert.equal(input.transcription.model, "realtime-transcribe");
  assert.equal(typeof input.transcription.prompt, "string");
}

function assertError(event, correlation, context) {
  assertExactKeys(event, ["error", "event_id", "type"], context);
  assertNonemptyString(event.event_id, `${context}.event_id`);
  assert.equal(event.type, "error");
  for (const field of Object.keys(event.error)) {
    assert.ok(["code", "event_id", "message", "param", "type"].includes(field));
  }
  for (const field of ["type", "code", "message"]) {
    assertNonemptyString(event.error[field], `${context}.error.${field}`);
  }
  if ("param" in event.error) {
    assert.ok(event.error.param === null || typeof event.error.param === "string");
  }
  if (correlation === undefined) {
    assert.equal("event_id" in event.error, false);
  } else {
    assert.equal(event.error.event_id, correlation);
  }
}

function canonicalBase64ByteLength(value, context) {
  assert.equal(typeof value, "string", `${context} Base64 is a string`);
  assert.match(
    value,
    /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/,
    `${context} is canonical Base64`,
  );
  const decoded = Buffer.from(value, "base64");
  assert.equal(decoded.toString("base64"), value, `${context} round-trips as Base64`);
  return decoded.length;
}

function assertValidCommitAudio(name, events) {
  let bufferedAudioBytes = 0;
  for (const [index, entry] of events.entries()) {
    const { message } = entry;
    if (entry.direction === "client") {
      if (message.type === "input_audio_buffer.append") {
        bufferedAudioBytes += canonicalBase64ByteLength(
          message.audio,
          `${name}[${index}].message.audio`,
        );
      } else if (message.type === "input_audio_buffer.commit") {
        assert.ok(
          bufferedAudioBytes >= minimumCommitAudioBytes,
          `${name}[${index}] commits ${bufferedAudioBytes} PCM16 bytes, below 100 ms`,
        );
      }
    } else if (
      message.type === "input_audio_buffer.committed" ||
      message.type === "input_audio_buffer.cleared"
    ) {
      bufferedAudioBytes = 0;
    }
  }
}

function assertServerEventFields(event, context) {
  const fieldsByType = {
    "session.created": ["event_id", "session", "type"],
    "session.updated": ["event_id", "session", "type"],
    "input_audio_buffer.committed": [
      "event_id",
      "item_id",
      "previous_item_id",
      "type",
    ],
    "input_audio_buffer.cleared": ["event_id", "type"],
    "conversation.item.created": ["event_id", "item", "previous_item_id", "type"],
    "conversation.item.input_audio_transcription.delta": [
      "content_index",
      "delta",
      "event_id",
      "item_id",
      "type",
    ],
    "conversation.item.input_audio_transcription.completed": [
      "content_index",
      "event_id",
      "item_id",
      "transcript",
      "type",
      "usage",
    ],
    "conversation.item.input_audio_transcription.failed": [
      "content_index",
      "error",
      "event_id",
      "item_id",
      "type",
    ],
    "conversation.item.input_audio_transcription.hypothesis": [
      "agreed",
      "audio_end_ms",
      "audio_start_ms",
      "content_index",
      "event_id",
      "finalized",
      "item_id",
      "revision",
      "tentative",
      "transcript",
      "type",
    ],
    error: ["error", "event_id", "type"],
  };
  assertExactKeys(event, fieldsByType[event.type], context);
  if (event.type === "session.created" || event.type === "session.updated") {
    assertSession(event.session, `${context}.session`);
  }
  if ("content_index" in event) assert.equal(event.content_index, 0);
  if ("previous_item_id" in event) {
    assert.ok(
      event.previous_item_id === null ||
        (typeof event.previous_item_id === "string" && event.previous_item_id.length > 0),
    );
  }
  if (event.type === "conversation.item.created") {
    assertExactKeys(event.item, ["content", "id", "role", "status", "type"], `${context}.item`);
    assertNonemptyString(event.item.id, `${context}.item.id`);
    assert.equal(event.item.type, "message");
    assert.equal(event.item.status, "completed");
    assert.equal(event.item.role, "user");
    assert.deepEqual(event.item.content, [{ type: "input_audio", transcript: null }]);
  }
}

test("canonical Realtime event fixtures match the Rust case list unchanged", async () => {
  assert.deepEqual((await readdir(fixtureDir)).sort(), fixtureFiles);

  const clients = await fixture("client-events.json");
  assert.deepEqual(sortedKeys(clients), clientCases);
  assert.equal(clients.session_update.type, "session.update");
  assert.equal(clients.input_audio_buffer_append.type, "input_audio_buffer.append");
  assert.equal(clients.input_audio_buffer_commit.type, "input_audio_buffer.commit");
  assert.equal(clients.input_audio_buffer_clear.type, "input_audio_buffer.clear");
  assertExactKeys(clients.session_update, ["event_id", "session", "type"], "session update");
  assertExactKeys(
    clients.input_audio_buffer_append,
    ["audio", "event_id", "type"],
    "append",
  );
  assertExactKeys(clients.input_audio_buffer_commit, ["type"], "commit");
  assertExactKeys(clients.input_audio_buffer_clear, ["type"], "clear");
  assertExactKeys(
    clients.session_update.session,
    ["audio", "include", "type"],
    "session update body",
  );
  assert.equal(clients.session_update.session.type, "transcription");
  assertExactKeys(clients.session_update.session.audio, ["input"], "session update audio");
  assertExactKeys(
    clients.session_update.session.audio.input,
    ["format", "noise_reduction", "transcription", "turn_detection"],
    "session update input",
  );

  const sessions = await fixture("effective-sessions.json");
  assert.deepEqual(sortedKeys(sessions), ["default", "updated"]);
  assertSession(sessions.default, "default session");
  assertSession(sessions.updated, "updated session");

  const servers = await fixture("server-events.json");
  assert.deepEqual(sortedKeys(servers), serverCases);
  for (const [name, event] of Object.entries(servers)) {
    assertNonemptyString(event.event_id, `${name}.event_id`);
    assertNonemptyString(event.type, `${name}.type`);
    assertServerEventFields(event, name);
  }
  assert.deepEqual(servers.session_created.session, sessions.default);
  assert.deepEqual(servers.session_updated.session, sessions.updated);
  assertError(servers.error_correlated, "client_bad_update", "correlated error");
  assertError(servers.error_minimal, undefined, "minimal error");
  assertError(servers.error_uncorrelated, null, "uncorrelated error");
  assert.deepEqual(sortedKeys(servers.transcription_completed.usage), ["seconds", "type"]);
  assert.equal(servers.transcription_completed.usage.type, "duration");
  assert.ok(servers.transcription_completed.usage.seconds >= 0);

  const hypothesis = servers.transcription_hypothesis;
  assert.equal(
    hypothesis.finalized + hypothesis.agreed + hypothesis.tentative,
    hypothesis.transcript,
  );
  assert.ok(Number.isSafeInteger(hypothesis.revision) && hypothesis.revision >= 0);
  assert.ok(hypothesis.audio_start_ms >= 0);
  assert.ok(hypothesis.audio_end_ms >= hypothesis.audio_start_ms);
});

test("canonical Realtime sequences cover every frozen contract path", async () => {
  const valid = await fixture("valid-sequences.json");
  assert.deepEqual(sortedKeys(valid), validSequenceCases);
  for (const [name, sequence] of Object.entries(valid)) {
    assertExactKeys(sequence, ["events", "invariants"], name);
    assert.ok(sequence.events.length > 0, `${name} has events`);
    assert.ok(sequence.invariants.length > 0, `${name} has invariants`);
    for (const entry of sequence.events) {
      assertExactKeys(entry, ["direction", "message"], `${name} entry`);
      assert.ok(entry.direction === "client" || entry.direction === "server");
      assertNonemptyString(entry.message.type, `${name} event type`);
      if (entry.direction === "server" || "event_id" in entry.message) {
        assertNonemptyString(entry.message.event_id, `${name} event ID`);
      }
      if (entry.direction === "server") {
        assertServerEventFields(entry.message, `${name} server event`);
      }
    }
    assertValidCommitAudio(name, sequence.events);
  }
  assert.deepEqual(
    valid.hypothesis_negotiation.events
      .filter(
        ({ message }) =>
          message.type === "conversation.item.input_audio_transcription.hypothesis",
      )
      .map(({ message }) => message.revision),
    [1, 2],
  );

  const invalid = await fixture("invalid-sequences.json");
  assert.deepEqual(sortedKeys(invalid), invalidSequenceCases);
  for (const [name, sequence] of Object.entries(invalid)) {
    assertExactKeys(
      sequence,
      [
        "effective_session_after",
        "expected_error",
        "input",
        "keeps_connection_usable",
      ],
      name,
    );
    assert.equal(sequence.keeps_connection_usable, true);
    assert.ok(
      sequence.effective_session_after === "default" ||
        sequence.effective_session_after === "updated",
    );
    let correlation;
    if ("message" in sequence.input) {
      correlation =
        typeof sequence.input.message.event_id === "string"
          ? sequence.input.message.event_id
          : sequence.input.message.event_id === undefined
            ? undefined
            : null;
    }
    assertError(sequence.expected_error, correlation, `${name} expected error`);
    assert.ok("message" in sequence.input || "wire_text" in sequence.input);
  }
});
