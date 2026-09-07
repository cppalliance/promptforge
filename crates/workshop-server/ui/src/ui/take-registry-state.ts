import type {
  MutableRegistry,
  PendingWireRequest,
  Reduction,
  RegistryTake,
  TakeRegistry,
} from "./take-registry-types";

/** Clones every registry collection for one immutable transition. */
export function cloneRegistry(state: TakeRegistry): MutableRegistry {
  return {
    takes: state.takes.map((take) => ({ ...take })),
    awaitingCommit: state.awaitingCommit.map((expectation) => ({ ...expectation })),
    retiredItemIds: [...state.retiredItemIds],
    clientEvents: state.clientEvents.map((binding) => ({ ...binding })),
    pendingWire: state.pendingWire.map((request) => ({ ...request })),
    activeTakeId: state.activeTakeId,
    capture: state.capture,
    stoppingTakeId: state.stoppingTakeId,
    connection: state.connection,
    nextTakeId: state.nextTakeId,
    nextRequestId: state.nextRequestId,
  };
}

/** Reserves one typed request correlation identifier. */
export function reserveWireRequest(
  state: MutableRegistry,
  command: PendingWireRequest["command"],
  takeId: number,
): number {
  const id = state.nextRequestId;
  state.nextRequestId += 1;
  state.pendingWire.push({ id, command, takeId });
  return id;
}

/** Restores all owned regions in reverse document order. */
export function rollbackAll(reduction: Reduction): void {
  const takeIds = reduction.state.takes.map((take) => take.id).reverse();
  for (const takeId of takeIds) {
    rollbackTake(reduction, takeId);
  }
}

/** Restores and retires one owned region. */
export function rollbackTake(reduction: Reduction, takeId: number): void {
  const take = takeById(reduction.state, takeId);
  if (take === null) {
    return;
  }
  replaceTake(reduction, take.id, take.original, "");
  removeTake(reduction, take.id);
}

/** Replaces one region and shifts every later region by the exact coordinate delta. */
export function replaceTake(
  reduction: Reduction,
  takeId: number,
  text: string,
  deltaText: string,
): void {
  const index = reduction.state.takes.findIndex((take) => take.id === takeId);
  if (index < 0) {
    return;
  }
  const take = reduction.state.takes[index];
  const oldEnd = take.to;
  const nextEnd = take.from + text.length;
  const delta = nextEnd - oldEnd;
  reduction.effects.push({
    domain: "editor",
    command: "replace",
    from: take.from,
    to: oldEnd,
    text,
  });
  reduction.state.takes[index] = {
    ...take,
    to: nextEnd,
    text,
    deltaText,
  };
  if (delta === 0) {
    return;
  }
  reduction.state.takes = reduction.state.takes.map((other) =>
    other.id !== take.id && other.from >= oldEnd
      ? { ...other, from: other.from + delta, to: other.to + delta }
      : other,
  );
}

/** Removes one take while retaining any outstanding acknowledgment owner. */
export function removeTake(reduction: Reduction, takeId: number): void {
  const take = takeById(reduction.state, takeId);
  if (take === null) {
    return;
  }
  reduction.state.takes = reduction.state.takes.filter(
    (candidate) => candidate.id !== takeId,
  );
  reduction.state.awaitingCommit = reduction.state.awaitingCommit.map(
    (expectation) =>
      expectation.takeId === takeId
        ? { takeId: null, itemId: take.itemId }
        : expectation,
  );
  if (take.itemId !== null) {
    retireItem(reduction.state, take.itemId);
  }
  if (reduction.state.activeTakeId === takeId) {
    reduction.state.activeTakeId = null;
  }
  reduction.state.clientEvents = reduction.state.clientEvents.filter(
    (binding) => binding.takeId !== takeId,
  );
  if (reduction.state.takes.length === 0) {
    reduction.effects.push({
      domain: "editor",
      command: "read-only",
      readOnly: false,
    });
  }
}

/** Applies the target-owned separator once to a transcript. */
export function composeTranscript(take: RegistryTake, transcript: string): string {
  return take.compositionPrefix !== "" &&
    transcript !== "" &&
    !/^\s/.test(transcript)
    ? take.compositionPrefix + transcript
    : transcript;
}

/** Binds one trusted server item identifier to its take. */
export function bindItem(
  state: MutableRegistry,
  takeId: number,
  itemId: string,
): void {
  const index = state.takes.findIndex((take) => take.id === takeId);
  if (index < 0 || state.retiredItemIds.includes(itemId)) {
    return;
  }
  state.takes[index] = { ...state.takes[index], itemId };
}

/** Records one item identifier as permanently unable to mutate a take. */
export function retireItem(state: MutableRegistry, itemId: string): void {
  if (!state.retiredItemIds.includes(itemId)) {
    state.retiredItemIds.push(itemId);
  }
}

/** Returns the active take when its owner still exists. */
export function activeTake(state: MutableRegistry): RegistryTake | null {
  return state.activeTakeId === null
    ? null
    : takeById(state, state.activeTakeId);
}

/** Finds one take by local identifier. */
export function takeById(
  state: MutableRegistry,
  takeId: number,
): RegistryTake | null {
  return state.takes.find((take) => take.id === takeId) ?? null;
}

/** Finds one take by trusted server item identifier. */
export function takeByItem(
  state: MutableRegistry,
  itemId: string,
): RegistryTake | null {
  return state.takes.find((take) => take.itemId === itemId) ?? null;
}
