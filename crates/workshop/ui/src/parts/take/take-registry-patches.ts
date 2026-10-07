// Word-level minimal patches from a take's shown text to its next text.
// Both texts split into alternating word and whitespace runs; the common
// leading and trailing runs are kept, the middle is aligned by longest
// common subsequence, and changed spans separated only by unchanged
// whitespace merge into one patch. A middle past the cell budget becomes
// one patch, so every update costs bounded time.

/** One edit in the earlier text's offsets. */
export interface WordPatch {
  readonly from: number;
  readonly to: number;
  readonly text: string;
}

/** Changed runs: old[oldStart, oldEnd) becomes next[newStart, newEnd). */
interface RunSpan {
  readonly oldStart: number;
  readonly oldEnd: number;
  readonly newStart: number;
  readonly newEnd: number;
}

const RUNS = /\s+|\S+/gu;
const WHITESPACE_RUN = /^\s/u;

// About 128 words a side; a longer changed middle is replaced whole.
const MAX_ALIGNMENT_CELLS = 65_536;

/** The patches that turn `before` into `after`, in document order. */
export function wordPatches(before: string, after: string): WordPatch[] {
  if (before === after) {
    return [];
  }
  const old = before.match(RUNS) ?? [];
  const next = after.match(RUNS) ?? [];
  let head = 0;
  while (head < old.length && head < next.length && old[head] === next[head]) {
    head += 1;
  }
  let oldEnd = old.length;
  let newEnd = next.length;
  while (oldEnd > head && newEnd > head && old[oldEnd - 1] === next[newEnd - 1]) {
    oldEnd -= 1;
    newEnd -= 1;
  }
  const oldOffsets = runOffsets(old);
  const newOffsets = runOffsets(next);
  const spans = changedSpans(old, next, head, oldEnd, newEnd);
  return mergeAcrossWhitespace(spans, old).map((span) => ({
    from: oldOffsets[span.oldStart] ?? before.length,
    to: oldOffsets[span.oldEnd] ?? before.length,
    text: after.slice(
      newOffsets[span.newStart] ?? after.length,
      newOffsets[span.newEnd] ?? after.length,
    ),
  }));
}

/** Each run's start offset, then the text's length. */
function runOffsets(runs: readonly string[]): number[] {
  const offsets = [0];
  let offset = 0;
  for (const run of runs) {
    offset += run.length;
    offsets.push(offset);
  }
  return offsets;
}

/** The changed spans of old[start, oldEnd) against next[start, newEnd). */
function changedSpans(
  old: readonly string[],
  next: readonly string[],
  start: number,
  oldEnd: number,
  newEnd: number,
): RunSpan[] {
  const rows = oldEnd - start;
  const columns = newEnd - start;
  if (rows === 0 && columns === 0) {
    return [];
  }
  if (rows === 0 || columns === 0 || rows * columns > MAX_ALIGNMENT_CELLS) {
    return [{ oldStart: start, oldEnd, newStart: start, newEnd }];
  }
  // common(i, j): the longest common subsequence of the runs after i and j.
  const width = columns + 1;
  const table = new Uint32Array((rows + 1) * width);
  const common = (i: number, j: number): number => table[i * width + j] ?? 0;
  for (let i = rows - 1; i >= 0; i -= 1) {
    for (let j = columns - 1; j >= 0; j -= 1) {
      table[i * width + j] =
        old[start + i] === next[start + j]
          ? common(i + 1, j + 1) + 1
          : Math.max(common(i + 1, j), common(i, j + 1));
    }
  }
  const spans: RunSpan[] = [];
  let open: { readonly oldStart: number; readonly newStart: number } | null = null;
  let i = 0;
  let j = 0;
  while (i < rows || j < columns) {
    if (i < rows && j < columns && old[start + i] === next[start + j]) {
      if (open !== null) {
        spans.push({ ...open, oldEnd: start + i, newEnd: start + j });
        open = null;
      }
      i += 1;
      j += 1;
      continue;
    }
    open ??= { oldStart: start + i, newStart: start + j };
    if (j === columns || (i < rows && common(i + 1, j) >= common(i, j + 1))) {
      i += 1;
    } else {
      j += 1;
    }
  }
  if (open !== null) {
    spans.push({ ...open, oldEnd, newEnd });
  }
  return spans;
}

/** Joins changed spans whose unchanged gap holds only whitespace runs. */
function mergeAcrossWhitespace(spans: readonly RunSpan[], old: readonly string[]): RunSpan[] {
  const merged: RunSpan[] = [];
  for (const span of spans) {
    const previous = merged.at(-1);
    if (
      previous !== undefined &&
      old.slice(previous.oldEnd, span.oldStart).every((run) => WHITESPACE_RUN.test(run))
    ) {
      merged[merged.length - 1] = { ...previous, oldEnd: span.oldEnd, newEnd: span.newEnd };
    } else {
      merged.push(span);
    }
  }
  return merged;
}
