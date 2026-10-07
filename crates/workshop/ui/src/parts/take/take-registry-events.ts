// Server-event and failure handling for the take registry. A decoded
// Realtime event reaches its take through the server's item id, which
// binds when the server acknowledges a commit - commits are matched to
// takes first in, first out - or, for a transcript under an unknown id,
// only when exactly one take is unbound. A hypothesis at or below the
// last revision applied for its item is dropped, so a repeated or
// reordered snapshot never replaces newer text. A hypothesis renders its
// finalized and agreed parts whole and its tentative part less the last
// whole word, which waits for the next update or `completed`; what shows
// of the tentative part is marked as the take's tentative tail. Each
// newly finished stable sentence is announced once, and `completed`
// announces whatever of the final transcript was not. A failed
// transcription keeps the visible text; connection loss lands each take's
// shown text less its tentative tail; service errors roll takes back.
// Status labels are local wording: server error text never reaches the
// status bar.

import type { RealtimeEvent } from "../../services/realtime-event-decoder";
import {
  activeTake,
  bindItem,
  composeTranscript,
  isRetiredItem,
  keepAll,
  landTake,
  recordAnnouncedSentences,
  recordHypothesisRevision,
  removeTake,
  replaceTake,
  reserveWireRequest,
  retireItem,
  rollbackTake,
  takeById,
  takeByItem,
} from "./take-registry-state";
import type { Reduction } from "./take-registry-types";

/** Tentative words hidden from the end of each hypothesis. */
const HELD_BACK_TENTATIVE_WORDS = 1;

const LAST_WORD = /\s*\S+\s*$/u;

/** A sentence end: a run of `.`, `?`, or `!` that whitespace or the end of the text follows. */
const SENTENCE_END = /[.?!]+(?=\s|$)/gu;

type SnapshotEvent = Extract<
  RealtimeEvent,
  {
    type:
      | "conversation.item.input_audio_transcription.hypothesis"
      | "conversation.item.input_audio_transcription.delta";
  }
>;

const UNAVAILABLE_LABEL = "Dictation is temporarily unavailable. Try again.";
const PRESERVED_TRANSCRIPTION_FAILED_LABEL =
  "Dictation could not be fully transcribed. Visible text was kept and can be edited.";
const PRECOMMIT_FAILED_LABEL =
  "Dictation stopped because transcription failed. Captured audio is being finalized.";

/** Applies one trusted decoded server event to the registry. */
export function serverEvent(reduction: Reduction, event: RealtimeEvent): void {
  switch (event.type) {
    case "session.created":
      return;
    case "session.updated":
      reduction.state.connection = "ready";
      return;
    case "input_audio_buffer.committed":
      acknowledgeCommit(reduction, event.item_id);
      return;
    case "input_audio_buffer.cleared":
    case "conversation.item.created":
      return;
    case "conversation.item.input_audio_transcription.hypothesis":
    case "conversation.item.input_audio_transcription.delta":
      applySnapshot(reduction, event);
      return;
    case "conversation.item.input_audio_transcription.completed":
      completeTake(reduction, event.item_id, event.transcript);
      return;
    case "conversation.item.input_audio_transcription.failed": {
      const take = takeByItem(reduction.state, event.item_id);
      if (take === null) {
        return;
      }
      preserveFailedTake(reduction, take.id);
      reduction.effects.push({
        domain: "status",
        command: "local",
        label: PRESERVED_TRANSCRIPTION_FAILED_LABEL,
        severity: "error",
      });
      return;
    }
    case "error":
      if (event.error.code === "too_much_unfinalized_audio") {
        retainedAudioOverload(reduction, event.error.event_id ?? null);
      } else if (event.error.code === "precommit_transcription_failed") {
        precommitTranscriptionFailure(reduction, event.error.event_id ?? null);
      } else {
        serviceError(reduction, event.error.event_id ?? null);
      }
      return;
    default: {
      const exhaustive: never = event;
      return exhaustive;
    }
  }
}

function precommitTranscriptionFailure(
  reduction: Reduction,
  eventId: string | null,
): void {
  if (eventId === null) {
    return;
  }
  const binding = reduction.state.clientEvents.find(
    (candidate) =>
      candidate.eventId === eventId && candidate.command === "append",
  );
  const take =
    binding === undefined ? null : takeById(reduction.state, binding.takeId);
  if (
    take === null ||
    reduction.state.activeTakeId !== take.id ||
    reduction.state.capture !== "recording"
  ) {
    return;
  }
  reduction.state.capture = "stopping";
  reduction.state.stoppingTakeId = take.id;
  reduction.effects.push(
    {
      domain: "capture",
      command: "stop",
      takeId: take.id,
      generation: reduction.state.activeGeneration,
    },
    { domain: "status", command: "recording", recording: false },
    {
      domain: "status",
      command: "local",
      label: PRECOMMIT_FAILED_LABEL,
      severity: "error",
    },
  );
}

function retainedAudioOverload(
  reduction: Reduction,
  eventId: string | null,
): void {
  const takeId =
    eventId === null
      ? reduction.state.activeTakeId
      : reduction.state.clientEvents.find((binding) => binding.eventId === eventId)
          ?.takeId ?? null;
  const take = takeId === null ? null : takeById(reduction.state, takeId);
  if (
    take !== null &&
    reduction.state.activeTakeId === take.id &&
    reduction.state.capture === "recording"
  ) {
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = take.id;
    reduction.effects.push(
      {
        domain: "capture",
        command: "stop",
        takeId: take.id,
        generation: reduction.state.activeGeneration,
      },
      { domain: "status", command: "recording", recording: false },
      {
        domain: "status",
        command: "local",
        label:
          "Dictation stopped because transcription could not keep up. Captured audio is being finalized.",
        severity: "error",
      },
    );
    return;
  }
  if (take === null) {
    reduction.effects.push({
      domain: "status",
      command: "local",
      label:
        "Dictation stopped because transcription could not keep up. Captured audio is being finalized.",
      severity: "error",
    });
  }
}

function preserveFailedTake(reduction: Reduction, takeId: number): void {
  if (
    reduction.state.activeTakeId === takeId &&
    reduction.state.capture === "recording"
  ) {
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = takeId;
    reduction.effects.push(
      {
        domain: "capture",
        command: "stop",
        takeId,
        generation: reduction.state.activeGeneration,
      },
      { domain: "status", command: "recording", recording: false },
    );
  }
  removeTake(reduction, takeId);
}

function acknowledgeCommit(reduction: Reduction, itemId: string): void {
  const boundTake = takeByItem(reduction.state, itemId);
  if (boundTake !== null) {
    const ownerIndex = reduction.state.awaitingCommit.findIndex(
      (expectation) =>
        expectation.generation === reduction.state.activeGeneration &&
        expectation.takeId === boundTake.id,
    );
    if (ownerIndex >= 0) {
      reduction.state.awaitingCommit.splice(ownerIndex, 1);
    }
    return;
  }
  const tombstoneIndex = reduction.state.awaitingCommit.findIndex(
    (expectation) =>
      expectation.generation === reduction.state.activeGeneration &&
      expectation.takeId === null &&
      expectation.itemId === itemId,
  );
  if (tombstoneIndex >= 0) {
    reduction.state.awaitingCommit.splice(tombstoneIndex, 1);
    return;
  }
  if (isRetiredItem(reduction.state, reduction.state.activeGeneration, itemId)) {
    return;
  }
  if (reduction.state.awaitingCommit.length === 0) {
    const active = activeTake(reduction.state);
    if (active !== null && active.itemId === null) {
      bindItem(reduction.state, active.id, itemId);
      return;
    }
    retireItem(reduction.state, reduction.state.activeGeneration, itemId);
    reduction.effects.push({
      domain: "status",
      command: "local",
      label: UNAVAILABLE_LABEL,
      severity: "error",
    });
    return;
  }

  const expectation = reduction.state.awaitingCommit.shift();
  if (expectation === undefined) {
    retireItem(reduction.state, reduction.state.activeGeneration, itemId);
    return;
  }
  if (expectation.takeId === null) {
    retireItem(reduction.state, expectation.generation, itemId);
    return;
  }
  const take = takeById(reduction.state, expectation.takeId);
  if (take === null) {
    retireItem(reduction.state, expectation.generation, itemId);
    return;
  }
  if (take.itemId === null) {
    bindItem(reduction.state, take.id, itemId);
    return;
  }
  if (take.itemId === itemId) {
    return;
  }
  retireItem(reduction.state, reduction.state.activeGeneration, itemId);
  rollbackTake(reduction, take.id);
  reduction.effects.push({
    domain: "status",
    command: "local",
    label: UNAVAILABLE_LABEL,
    severity: "error",
  });
}

/** Applies a hypothesis snapshot, or appends a delta. */
function applySnapshot(reduction: Reduction, event: SnapshotEvent): void {
  const itemId = event.item_id;
  let take = takeByItem(reduction.state, itemId);
  if (take === null) {
    if (
      isRetiredItem(reduction.state, reduction.state.activeGeneration, itemId) ||
      reduction.state.awaitingCommit.some(
        (expectation) =>
          expectation.generation === reduction.state.activeGeneration &&
          expectation.takeId === null &&
          expectation.itemId === null,
      )
    ) {
      return;
    }
    const unbound = reduction.state.takes.filter(
      (candidate) => candidate.itemId === null,
    );
    const candidate = unbound[0];
    if (unbound.length !== 1 || candidate === undefined) {
      return;
    }
    bindItem(reduction.state, candidate.id, itemId);
    take = takeById(reduction.state, candidate.id);
  }
  if (take === null) {
    return;
  }
  if (event.type === "conversation.item.input_audio_transcription.delta") {
    const transcript = take.deltaText + event.delta;
    replaceTake(reduction, take.id, composeTranscript(take, transcript), transcript);
    announceSentences(reduction, take.id, transcript);
    return;
  }
  if (take.hypothesisRevision !== null && event.revision <= take.hypothesisRevision) {
    return;
  }
  recordHypothesisRevision(reduction.state, take.id, event.revision);
  const stable = event.finalized + event.agreed;
  const tentative = shownTentative(stable, event.tentative);
  replaceTake(
    reduction,
    take.id,
    composeTranscript(take, stable + tentative),
    event.transcript,
    tentative.length,
  );
  announceSentences(reduction, take.id, stable);
}

/** The offset just past each sentence end in a transcript. */
function sentenceEnds(transcript: string): number[] {
  return [...transcript.matchAll(SENTENCE_END)].map((end) => end.index + end[0].length);
}

/** Announces the finished sentences of a take's stable text it has not yet announced. */
function announceSentences(reduction: Reduction, takeId: number, stable: string): void {
  const take = takeById(reduction.state, takeId);
  const ends = sentenceEnds(stable);
  if (take === null || ends.length <= take.announcedSentences) {
    return;
  }
  const text = stable
    .slice(ends[take.announcedSentences - 1] ?? 0, ends[ends.length - 1])
    .trim();
  recordAnnouncedSentences(reduction.state, takeId, ends.length);
  if (text !== "") {
    reduction.effects.push({ domain: "status", command: "announce", text });
  }
}

/**
 * The tentative part less its last whole words. A word starts after
 * whitespace or at the start of the transcript; a fragment glued to the
 * stable text stays shown, since hiding it would cut a word in half.
 */
function shownTentative(stable: string, tentative: string): string {
  let shown = tentative;
  for (let held = 0; held < HELD_BACK_TENTATIVE_WORDS; held += 1) {
    const last = LAST_WORD.exec(shown);
    if (
      last === null ||
      (last.index === 0 && !/^\s/u.test(last[0]) && /\S$/u.test(stable))
    ) {
      break;
    }
    shown = shown.slice(0, last.index);
  }
  return shown;
}

function completeTake(
  reduction: Reduction,
  itemId: string,
  transcript: string,
): void {
  const take = takeByItem(reduction.state, itemId);
  if (take === null) {
    return;
  }
  if (
    reduction.state.activeTakeId === take.id &&
    reduction.state.capture === "recording"
  ) {
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = take.id;
    reduction.effects.push(
      {
        domain: "capture",
        command: "stop",
        takeId: take.id,
        generation: reduction.state.activeGeneration,
      },
      { domain: "status", command: "recording", recording: false },
    );
    reduction.state.activeTakeId = null;
  }
  const authoritative = transcript.trimEnd();
  const text = composeTranscript(take, authoritative);
  landTake(reduction, take.id, text, authoritative);
  // A final with fewer sentence ends than were announced resumes after its last one.
  const ends = sentenceEnds(authoritative);
  const unannounced = authoritative
    .slice(ends[Math.min(take.announcedSentences, ends.length) - 1] ?? 0)
    .trim();
  if (unannounced !== "") {
    reduction.effects.push({ domain: "status", command: "announce", text: unannounced });
  }
  removeTake(reduction, take.id);
  if (text === "") {
    reduction.effects.push({
      domain: "status",
      command: "local",
      label: "No speech was detected.",
      severity: "info",
    });
  } else {
    reduction.effects.push(
      { domain: "editor", command: "focus" },
      {
        domain: "status",
        command: "local",
        label: "Dictation ready.",
        severity: "info",
      },
    );
  }
}

/** Applies a locally classified service failure without trusting remote wording. */
export function serviceError(
  reduction: Reduction,
  eventId: string | null,
): void {
  const takeId =
    eventId === null
      ? reduction.state.activeTakeId
      : reduction.state.clientEvents.find((binding) => binding.eventId === eventId)
          ?.takeId ?? null;
  if (takeId !== null && takeById(reduction.state, takeId) !== null) {
    failTake(reduction, takeId);
    return;
  }
  reduction.effects.push({
    domain: "status",
    command: "local",
    label: UNAVAILABLE_LABEL,
    severity: "error",
  });
}

/**
 * Ends every take when the Realtime connection is lost: each keeps its
 * shown text less the tentative tail, lands it, and releases the editor.
 */
export function connectionLost(reduction: Reduction): void {
  reduction.state.connection = "unavailable";
  const activeTakeId = reduction.state.activeTakeId;
  if (activeTakeId !== null && reduction.state.capture !== "idle") {
    reduction.effects.push(
      { domain: "capture", command: "clear" },
      {
        domain: "capture",
        command: "stop",
        takeId: activeTakeId,
        generation: reduction.state.activeGeneration,
      },
    );
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = activeTakeId;
  }
  keepAll(reduction);
  reduction.state.awaitingCommit = [];
  reduction.state.pendingWire = [];
  reduction.state.clientEvents = [];
  reduction.state.retiredItems = [];
  reduction.effects.push(
    { domain: "status", command: "recording", recording: false },
    {
      domain: "status",
      command: "local",
      label: UNAVAILABLE_LABEL,
      severity: "error",
    },
  );
}

/** Rolls one failed take back through typed capture, wire, editor, and status effects. */
export function failTake(reduction: Reduction, takeId: number): void {
  if (
    reduction.state.activeTakeId === takeId &&
    reduction.state.capture !== "idle"
  ) {
    reduction.effects.push(
      { domain: "capture", command: "clear" },
      {
        domain: "capture",
        command: "stop",
        takeId,
        generation: reduction.state.activeGeneration,
      },
      {
        domain: "wire",
        command: "clear",
        generation: reduction.state.activeGeneration,
      },
      { domain: "status", command: "recording", recording: false },
    );
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = takeId;
  }
  rollbackTake(reduction, takeId);
  reduction.effects.push({
    domain: "status",
    command: "local",
    label: UNAVAILABLE_LABEL,
    severity: "error",
  });
}
