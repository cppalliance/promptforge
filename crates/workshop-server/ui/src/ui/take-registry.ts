import type { SttInsertionContext } from "./stt";
import {
  connectionLost,
  failTake,
  serverEvent,
} from "./take-registry-events";
import {
  activeTake,
  cloneRegistry,
  reserveWireRequest,
  rollbackAll,
  rollbackTake,
  takeById,
} from "./take-registry-state";
import type {
  Reduction,
  RegistryTake,
  TakeRegistry,
  TakeRegistryInput,
  TakeRegistryTransition,
} from "./take-registry-types";

export type {
  RegistryTake,
  TakeRegistry,
  TakeRegistryCaptureEffect,
  TakeRegistryEditorEffect,
  TakeRegistryEffect,
  TakeRegistryInput,
  TakeRegistryStatusEffect,
  TakeRegistryTransition,
  TakeRegistryWireEffect,
} from "./take-registry-types";

/** Creates an empty registry ready for its first take. */
export function createTakeRegistry(): TakeRegistry {
  return {
    takes: [],
    awaitingCommit: [],
    retiredItemIds: [],
    clientEvents: [],
    pendingWire: [],
    activeTakeId: null,
    capture: "idle",
    stoppingTakeId: null,
    connection: "ready",
    nextTakeId: 1,
    nextRequestId: 1,
  };
}

/** Applies one input without performing editor, capture, status, or wire work. */
export function reduceTakeRegistry(
  current: TakeRegistry,
  input: TakeRegistryInput,
): TakeRegistryTransition {
  const reduction: Reduction = {
    state: cloneRegistry(current),
    effects: [],
  };
  switch (input.type) {
    case "user.start":
      startTake(reduction, input.context);
      break;
    case "user.stop":
      stopTake(reduction);
      break;
    case "user.discard":
      discardTakes(reduction);
      break;
    case "capture.audio":
      appendAudio(reduction, input.chunk);
      break;
    case "capture.stopped":
      captureStopped(reduction, input.takeId, input.ok);
      break;
    case "wire.result":
      wireResult(reduction, input.requestId, input.eventId);
      break;
    case "server.event":
      serverEvent(reduction, input.event);
      break;
    case "connection.lost":
      connectionLost(reduction);
      break;
    case "connection.ready":
      reduction.state.connection = "ready";
      break;
    default: {
      const exhaustive: never = input;
      return exhaustive;
    }
  }
  return reduction;
}

function startTake(reduction: Reduction, context: SttInsertionContext): void {
  if (
    reduction.state.activeTakeId !== null ||
    reduction.state.capture !== "idle" ||
    reduction.state.connection !== "ready"
  ) {
    return;
  }
  const id = reduction.state.nextTakeId;
  reduction.state.nextTakeId += 1;
  const take: RegistryTake = {
    id,
    from: context.range.start,
    to: context.range.end,
    original: context.original,
    compositionPrefix: context.compositionPrefix,
    itemId: null,
    text: context.original,
    deltaText: "",
  };
  const wasEmpty = reduction.state.takes.length === 0;
  reduction.state.takes.push(take);
  reduction.state.takes.sort((left, right) => left.from - right.from || left.id - right.id);
  reduction.state.activeTakeId = id;
  reduction.state.capture = "recording";
  if (wasEmpty) {
    reduction.effects.push({
      domain: "editor",
      command: "read-only",
      readOnly: true,
    });
  }
  reduction.effects.push(
    { domain: "status", command: "recording", recording: true },
    {
      domain: "status",
      command: "local",
      label: "Listening...",
      severity: "info",
    },
  );
}

function stopTake(reduction: Reduction): void {
  const take = activeTake(reduction.state);
  if (take === null || reduction.state.capture !== "recording") {
    return;
  }
  reduction.state.capture = "stopping";
  reduction.state.stoppingTakeId = take.id;
  reduction.effects.push(
    { domain: "capture", command: "stop", takeId: take.id },
    { domain: "status", command: "recording", recording: false },
    {
      domain: "status",
      command: "local",
      label: "Transcribing...",
      severity: "info",
    },
  );
}

function appendAudio(reduction: Reduction, chunk: ArrayBuffer): void {
  const take = activeTake(reduction.state);
  if (take === null || reduction.state.capture !== "recording") {
    return;
  }
  const requestId = reserveWireRequest(reduction.state, "append", take.id);
  reduction.effects.push({
    domain: "wire",
    command: "append",
    requestId,
    takeId: take.id,
    chunk,
  });
}

function captureStopped(reduction: Reduction, takeId: number, ok: boolean): void {
  if (
    reduction.state.capture !== "stopping" ||
    reduction.state.stoppingTakeId !== takeId
  ) {
    return;
  }
  reduction.state.capture = "idle";
  reduction.state.stoppingTakeId = null;
  if (reduction.state.activeTakeId === takeId) {
    reduction.state.activeTakeId = null;
  }
  const take = takeById(reduction.state, takeId);
  if (take === null) {
    return;
  }
  if (!ok) {
    reduction.effects.push({ domain: "wire", command: "clear" });
    rollbackTake(reduction, takeId);
    reduction.effects.push({
      domain: "status",
      command: "local",
      label: "Dictation could not finish capturing audio. Try again.",
      severity: "error",
    });
    return;
  }
  const requestId = reserveWireRequest(reduction.state, "commit", takeId);
  reduction.effects.push({
    domain: "wire",
    command: "commit",
    requestId,
    takeId,
  });
}

function discardTakes(reduction: Reduction): void {
  if (reduction.state.takes.length === 0) {
    return;
  }
  const activeTakeId = reduction.state.activeTakeId;
  if (activeTakeId !== null && reduction.state.capture !== "idle") {
    reduction.effects.push(
      { domain: "capture", command: "clear" },
      {
        domain: "capture",
        command: "stop",
        takeId: activeTakeId,
      },
      { domain: "wire", command: "clear" },
    );
    reduction.state.capture = "stopping";
    reduction.state.stoppingTakeId = activeTakeId;
  }
  rollbackAll(reduction);
  reduction.effects.push({
    domain: "status",
    command: "recording",
    recording: false,
  });
}

function wireResult(
  reduction: Reduction,
  requestId: number,
  eventId: string | null,
): void {
  const index = reduction.state.pendingWire.findIndex(
    (request) => request.id === requestId,
  );
  if (index < 0) {
    return;
  }
  const [request] = reduction.state.pendingWire.splice(index, 1);
  const take = takeById(reduction.state, request.takeId);
  if (eventId === null) {
    if (take !== null) {
      failTake(reduction, take.id);
    }
    return;
  }
  if (take === null) {
    if (request.command === "commit") {
      reduction.state.awaitingCommit.push({ takeId: null, itemId: null });
    }
    return;
  }
  reduction.state.clientEvents.push({ eventId, takeId: take.id });
  if (request.command === "commit") {
    reduction.state.awaitingCommit.push({
      takeId: take.id,
      itemId: take.itemId,
    });
  }
}
