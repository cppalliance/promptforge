import type { RealtimeEvent } from "../services/realtime-event-decoder";
import {
  activeTake,
  bindItem,
  composeTranscript,
  removeTake,
  replaceTake,
  reserveWireRequest,
  retireItem,
  rollbackAll,
  rollbackTake,
  takeById,
  takeByItem,
} from "./take-registry-state";
import type { Reduction } from "./take-registry-types";

const UNAVAILABLE_LABEL = "Dictation is temporarily unavailable. Try again.";
const TRANSCRIPTION_FAILED_LABEL = "Dictation could not be transcribed. Try again.";

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
      applySnapshot(reduction, event.item_id, event.transcript, false);
      return;
    case "conversation.item.input_audio_transcription.delta":
      applySnapshot(reduction, event.item_id, event.delta, true);
      return;
    case "conversation.item.input_audio_transcription.completed":
      completeTake(reduction, event.item_id, event.transcript);
      return;
    case "conversation.item.input_audio_transcription.failed": {
      const take = takeByItem(reduction.state, event.item_id);
      if (take !== null) {
        rollbackTake(reduction, take.id);
      }
      reduction.effects.push({
        domain: "status",
        command: "local",
        label: TRANSCRIPTION_FAILED_LABEL,
        severity: "error",
      });
      return;
    }
    case "error":
      serviceError(reduction, event.error.event_id ?? null);
      return;
    default: {
      const exhaustive: never = event;
      return exhaustive;
    }
  }
}

function acknowledgeCommit(reduction: Reduction, itemId: string): void {
  const boundTake = takeByItem(reduction.state, itemId);
  if (boundTake !== null) {
    const ownerIndex = reduction.state.awaitingCommit.findIndex(
      (expectation) => expectation.takeId === boundTake.id,
    );
    if (ownerIndex >= 0) {
      reduction.state.awaitingCommit.splice(ownerIndex, 1);
    }
    return;
  }
  const tombstoneIndex = reduction.state.awaitingCommit.findIndex(
    (expectation) =>
      expectation.takeId === null && expectation.itemId === itemId,
  );
  if (tombstoneIndex >= 0) {
    reduction.state.awaitingCommit.splice(tombstoneIndex, 1);
    return;
  }
  if (reduction.state.retiredItemIds.includes(itemId)) {
    return;
  }
  if (reduction.state.awaitingCommit.length === 0) {
    const active = activeTake(reduction.state);
    if (active !== null && active.itemId === null) {
      bindItem(reduction.state, active.id, itemId);
      return;
    }
    retireItem(reduction.state, itemId);
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
    retireItem(reduction.state, itemId);
    return;
  }
  if (expectation.takeId === null) {
    retireItem(reduction.state, itemId);
    return;
  }
  const take = takeById(reduction.state, expectation.takeId);
  if (take === null) {
    retireItem(reduction.state, itemId);
    return;
  }
  if (take.itemId === null) {
    bindItem(reduction.state, take.id, itemId);
    return;
  }
  if (take.itemId === itemId) {
    return;
  }
  retireItem(reduction.state, itemId);
  rollbackTake(reduction, take.id);
  reduction.effects.push({
    domain: "status",
    command: "local",
    label: UNAVAILABLE_LABEL,
    severity: "error",
  });
}

function applySnapshot(
  reduction: Reduction,
  itemId: string,
  incoming: string,
  append: boolean,
): void {
  let take = takeByItem(reduction.state, itemId);
  if (take === null) {
    if (
      reduction.state.retiredItemIds.includes(itemId) ||
      reduction.state.awaitingCommit.some(
        (expectation) =>
          expectation.takeId === null && expectation.itemId === null,
      )
    ) {
      return;
    }
    const unbound = reduction.state.takes.filter(
      (candidate) => candidate.itemId === null,
    );
    if (unbound.length !== 1) {
      return;
    }
    bindItem(reduction.state, unbound[0].id, itemId);
    take = takeById(reduction.state, unbound[0].id);
  }
  if (take === null) {
    return;
  }
  const transcript = append ? take.deltaText + incoming : incoming;
  replaceTake(reduction, take.id, composeTranscript(take, transcript), transcript);
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
      { domain: "capture", command: "stop", takeId: take.id },
      { domain: "status", command: "recording", recording: false },
    );
    reduction.state.activeTakeId = null;
  }
  const authoritative = transcript.trimEnd();
  const text = composeTranscript(take, authoritative);
  replaceTake(reduction, take.id, text, authoritative);
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

/** Rolls all live ownership back when the Realtime connection is lost. */
export function connectionLost(reduction: Reduction): void {
  reduction.state.connection = "unavailable";
  const activeTakeId = reduction.state.activeTakeId;
  if (activeTakeId !== null && reduction.state.capture !== "idle") {
    reduction.effects.push(
      { domain: "capture", command: "clear" },
      { domain: "capture", command: "stop", takeId: activeTakeId },
    );
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = activeTakeId;
  }
  rollbackAll(reduction);
  reduction.state.awaitingCommit = [];
  reduction.state.pendingWire = [];
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
      { domain: "capture", command: "stop", takeId },
      { domain: "wire", command: "clear" },
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
