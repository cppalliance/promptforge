// Take registry state helpers, called on the writable clone inside one
// reduction: cloning, lookup, item binding, and region edits.
// replaceTake replaces a take's captured range whole on its first write,
// since the selection may span structure its text does not show, then
// patches only the changed word runs, leaves the caret at the take's end
// as a whole-range write would, and shifts every later region by the
// edit's length delta so document offsets stay exact. Those writes are
// transient, outside the target's undo history; landing or rolling back
// first returns the region to its captured selection, in the target's own
// form when it supplied one, and landTake then writes a take's final text
// as the one edit that enters history. removeTake
// clears the take's tentative mark, leaves a tombstone for any commit the
// server has not yet acknowledged, and retires the take's item id, so
// late server events for it change nothing.

import { type WordPatch, wordPatches } from "./take-registry-patches";
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
    retiredItems: state.retiredItems.map((item) => ({ ...item })),
    clientEvents: state.clientEvents.map((binding) => ({ ...binding })),
    pendingWire: state.pendingWire.map((request) => ({ ...request })),
    activeTakeId: state.activeTakeId,
    capture: state.capture,
    stoppingTakeId: state.stoppingTakeId,
    connection: state.connection,
    activeGeneration: state.activeGeneration,
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
  state.pendingWire.push({
    id,
    generation: state.activeGeneration,
    command,
    takeId,
  });
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
  restoreTake(reduction, takeId);
  removeTake(reduction, takeId);
}

/**
 * Lands every owned region's shown text less its tentative tail, in
 * reverse document order, and retires it; a region left with no text
 * is restored instead.
 */
export function keepAll(reduction: Reduction): void {
  const takeIds = reduction.state.takes.map((take) => take.id).reverse();
  for (const takeId of takeIds) {
    const take = takeById(reduction.state, takeId);
    if (take === null) {
      continue;
    }
    const kept = take.written
      ? take.text.slice(0, take.text.length - take.tentativeLength).trimEnd()
      : "";
    if (kept === "") {
      rollbackTake(reduction, takeId);
      continue;
    }
    landTake(reduction, takeId, kept, take.deltaText);
    removeTake(reduction, takeId);
  }
}

/**
 * Writes one region whose last `tentativeLength` characters are tentative,
 * outside the target's undo history, and shifts every later region by the
 * exact coordinate delta.
 */
export function replaceTake(
  reduction: Reduction,
  takeId: number,
  text: string,
  deltaText: string,
  tentativeLength = 0,
): void {
  const take = takeById(reduction.state, takeId);
  if (take === null) {
    return;
  }
  const patches = take.written
    ? wordPatches(take.text, text)
    : [{ from: 0, to: take.to - take.from, text }];
  writeTake(reduction, take, text, deltaText, tentativeLength, patches, true);
}

/**
 * Lands a take's final text as the one edit that enters the target's
 * undo history. Interim writes stayed out of it, so a written region first
 * returns to its captured selection outside history; the final text then
 * replaces that selection whole, and undoing it brings the selection back
 * as the target held it.
 */
export function landTake(
  reduction: Reduction,
  takeId: number,
  text: string,
  deltaText: string,
): void {
  restoreTake(reduction, takeId);
  const take = takeById(reduction.state, takeId);
  if (take === null || text === take.original) {
    return;
  }
  const whole = [{ from: 0, to: take.to - take.from, text }];
  writeTake(reduction, take, text, deltaText, 0, whole, false);
}

/**
 * Returns a written region to the selection its take captured, outside
 * the target's undo history, and shifts every later region by the exact
 * coordinate delta. The target restores its own form of the selection
 * when it captured one, since the text alone flattens paragraph breaks
 * and mention chips. The region is then unwritten, so the next write
 * replaces it whole.
 */
function restoreTake(reduction: Reduction, takeId: number): void {
  const take = takeById(reduction.state, takeId);
  if (take === null || !take.written) {
    return;
  }
  const content = take.originalContent;
  if (content !== undefined) {
    reduction.effects.push({
      domain: "editor",
      command: "restore",
      from: take.from,
      to: take.to,
      content,
    });
  } else if (take.text !== take.original) {
    reduction.effects.push({
      domain: "editor",
      command: "replace",
      from: take.from,
      to: take.to,
      text: take.original,
      transient: true,
    });
  }
  if (take.tentativeLength > 0) {
    reduction.effects.push({ domain: "editor", command: "tentative", takeId, range: null });
  }
  const width = content === undefined ? take.original.length : take.originalWidth;
  settleTake(reduction, take, {
    ...take,
    to: take.from + width,
    text: take.original,
    written: false,
    tentativeLength: 0,
  });
}

function writeTake(
  reduction: Reduction,
  take: RegistryTake,
  text: string,
  deltaText: string,
  tentativeLength: number,
  patches: readonly WordPatch[],
  transient: boolean,
): void {
  const nextEnd = take.from + text.length;
  let shift = 0;
  let caret = nextEnd;
  for (const patch of patches) {
    const from = take.from + patch.from + shift;
    reduction.effects.push({
      domain: "editor",
      command: "replace",
      from,
      to: take.from + patch.to + shift,
      text: patch.text,
      transient,
    });
    shift += patch.text.length - (patch.to - patch.from);
    caret = from + patch.text.length;
  }
  // The next take's insertion context reads the caret, which each replace leaves after its insert.
  if (caret !== nextEnd) {
    reduction.effects.push({ domain: "editor", command: "caret", at: nextEnd });
  }
  // A patch under the mark can drop it in the target, so any patch resends it.
  if (
    (patches.length > 0 || tentativeLength !== take.tentativeLength) &&
    (tentativeLength > 0 || take.tentativeLength > 0)
  ) {
    reduction.effects.push({
      domain: "editor",
      command: "tentative",
      takeId: take.id,
      range: tentativeLength > 0 ? { from: nextEnd - tentativeLength, to: nextEnd } : null,
    });
  }
  settleTake(reduction, take, {
    ...take,
    to: nextEnd,
    text,
    deltaText,
    written: true,
    tentativeLength,
  });
}

/** Stores a take's next state and shifts every region past its old end by the change in its end. */
function settleTake(reduction: Reduction, take: RegistryTake, next: RegistryTake): void {
  const delta = next.to - take.to;
  reduction.state.takes = reduction.state.takes.map((other) => {
    if (other.id === take.id) {
      return next;
    }
    return delta !== 0 && other.from >= take.to
      ? { ...other, from: other.from + delta, to: other.to + delta }
      : other;
  });
}

/** Removes one take while retaining any outstanding acknowledgment owner. */
export function removeTake(reduction: Reduction, takeId: number): void {
  const take = takeById(reduction.state, takeId);
  if (take === null) {
    return;
  }
  if (take.tentativeLength > 0) {
    reduction.effects.push({
      domain: "editor",
      command: "tentative",
      takeId,
      range: null,
    });
  }
  reduction.state.takes = reduction.state.takes.filter(
    (candidate) => candidate.id !== takeId,
  );
  reduction.state.awaitingCommit = reduction.state.awaitingCommit.map(
    (expectation) =>
      expectation.takeId === takeId
        ? {
            generation: expectation.generation,
            takeId: null,
            itemId: take.itemId,
          }
        : expectation,
  );
  if (take.itemId !== null) {
    retireItem(reduction.state, take.itemGeneration ?? take.generation, take.itemId);
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
  const take = state.takes[index];
  if (
    index < 0 ||
    take === undefined ||
    isRetiredItem(state, state.activeGeneration, itemId)
  ) {
    return;
  }
  state.takes[index] = {
    ...take,
    itemId,
    itemGeneration: state.activeGeneration,
  };
}

/** Records the newest hypothesis revision applied to one take. */
export function recordHypothesisRevision(
  state: MutableRegistry,
  takeId: number,
  revision: number,
): void {
  const index = state.takes.findIndex((take) => take.id === takeId);
  const take = state.takes[index];
  if (index < 0 || take === undefined) {
    return;
  }
  state.takes[index] = { ...take, hypothesisRevision: revision };
}

/** Records how many of one take's finished stable sentences have been announced. */
export function recordAnnouncedSentences(
  state: MutableRegistry,
  takeId: number,
  announcedSentences: number,
): void {
  const index = state.takes.findIndex((take) => take.id === takeId);
  const take = state.takes[index];
  if (index < 0 || take === undefined) {
    return;
  }
  state.takes[index] = { ...take, announcedSentences };
}

/** Records one item identifier as permanently unable to mutate a take. */
export function retireItem(
  state: MutableRegistry,
  generation: number,
  itemId: string,
): void {
  if (!isRetiredItem(state, generation, itemId)) {
    state.retiredItems.push({ generation, itemId });
  }
}

/** Whether one item identity is retired in its originating generation. */
export function isRetiredItem(
  state: MutableRegistry,
  generation: number,
  itemId: string,
): boolean {
  return state.retiredItems.some(
    (item) => item.generation === generation && item.itemId === itemId,
  );
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
  return (
    state.takes.find(
      (take) =>
        take.itemId === itemId &&
        take.itemGeneration === state.activeGeneration,
    ) ?? null
  );
}
