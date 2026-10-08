// The agent transcript model: a pure, DOM-free function from the
// session's items to what the view paints - turns of keyed rows, and a
// tail status. `buildTranscript(items, generating, reconnecting)` is
// recomputed on every change; every row carries a stable key so the view
// can reconcile by key and update rows in place instead of rebuilding
// them. Keys are stable because they derive from ids the wire already
// stamped (the reply id, the call id, the item index), never from
// position among rows, and because items only ever append.
//
// The rules live here, in this order:
//
// - Turns: a user item starts a turn; a tool result attaches to its call
//   by id within its turn and never becomes a row.
// - Grouping: reasoning items and tool calls gather into an open group; a
//   reply, a user item, or an error closes it; ask-tool calls always
//   stand alone. A closed group with one tool call and nothing else is a
//   `tool` row, thinking only is a `thought` row, anything else a `group`.
// - Thought labels, group summaries, tool verbs, and loading (tool-labels.ts).
// - The tail status under the last row of a generating turn.
//
// Item text is untrusted model-era data; the model only carries it.

import type { ReasoningItem, ToolCallRow, TranscriptItem } from "../../../services/agent-session";
import {
  groupSummary,
  isToolLoading,
  toolKind,
  toolLabel,
  type GroupSummary,
  type ToolKind,
  type ToolLabel,
  type TurnState,
} from "./tool-labels";

// --- Types --------------------------------------------------------------------

/** A thought's header: the verb, and the details after it when there are any. */
export interface ThoughtLabel {
  readonly action: string;
  readonly details: string | null;
}

/** The slice of a thought a label reads. */
export interface ThoughtInput {
  readonly text: string;
  /** How long the thinking ran, or null when unknown. */
  readonly durationMs: number | null;
  /** True while the thinking is still running. */
  readonly streaming: boolean;
}

/** One reasoning item as a step: a thought row's entry or a group's thinking line. */
export interface ThoughtStep extends ThoughtInput {
  readonly step: "thought";
  readonly key: string;
  readonly label: ThoughtLabel;
}

/** One tool call as a step: a tool row's body or a group's tool line. */
export interface ToolStep {
  readonly step: "tool";
  readonly key: string;
  readonly call: ToolCallRow;
  readonly toolKind: ToolKind;
  /** The result text once the call's result landed, else null. */
  readonly result: string | null;
  readonly loading: boolean;
  readonly label: ToolLabel;
}

/** A step inside a group. */
export type GroupStep = ThoughtStep | ToolStep;

/** The operator's message. */
export interface HumanRow {
  readonly kind: "human";
  readonly key: string;
  readonly text: string;
}

/** An assistant reply, as markdown source. */
export interface MarkdownRow {
  readonly kind: "markdown";
  readonly key: string;
  readonly text: string;
  /** True while the reply is still streaming. */
  readonly streaming: boolean;
}

/** Thinking and nothing else: one or more consecutive thoughts. */
export interface ThoughtRow {
  readonly kind: "thought";
  readonly key: string;
  readonly thoughts: readonly ThoughtStep[];
  /** The thoughts' text, joined by a blank line: the row's body. */
  readonly text: string;
  readonly streaming: boolean;
  /** The sum of the thoughts' durations, or null when none is known. */
  readonly durationMs: number | null;
  readonly label: ThoughtLabel;
}

/** Thinking and tool steps that ran together. */
export interface GroupRow {
  readonly kind: "group";
  readonly key: string;
  readonly steps: readonly GroupStep[];
  readonly summary: GroupSummary;
  /** True while the group is the last row of the generating turn. */
  readonly active: boolean;
}

/** A standalone tool call: a lone call, or an ask. */
export interface ToolRow {
  readonly kind: "tool";
  readonly key: string;
  readonly step: ToolStep;
}

export type TranscriptRow = HumanRow | MarkdownRow | ThoughtRow | GroupRow | ToolRow;

/** One turn: a human row and what the agent did about it. */
export interface TranscriptTurn {
  readonly key: string;
  readonly rows: readonly TranscriptRow[];
}

/** What the tail status is about. */
export type TailKind = "reconnecting" | "tool" | "thinking" | "planning";

/** The status line under the last row while the agent works. */
export interface TailStatus {
  readonly kind: TailKind;
  /** The verb, shimmering. */
  readonly action: string;
  readonly details: string;
  /** The call name leading `details`, for the verb's color; see `ToolLabel`. */
  readonly callName: string | null;
  /** True when the last row is a group: the tail renders inside it. */
  readonly inGroup: boolean;
}

export interface TranscriptModel {
  readonly turns: readonly TranscriptTurn[];
  /** The tail status, or null when there is none to show. */
  readonly tail: TailStatus | null;
}

// --- Thought labels --------------------------------------------------------------

// No `*` inside the capture: a line like `**A** vs **B**` is body text, not one title.
const BOLD_TITLE = /^\*\*([^*]+)\*\*$/;
const HEADING_TITLE = /^#{1,6}\s+(.+)$/;

/** The title a line holds when it is `**title**` or `# title`, else null. */
function headerTitle(line: string): string | null {
  const match = BOLD_TITLE.exec(line) ?? HEADING_TITLE.exec(line);
  return match?.[1]?.trim() ?? null;
}

/**
 * Reads thinking text for title-only form: every non-blank line is a
 * title. `title` is the last one; `titleOnly` is false when any line is
 * body text or there is no title at all.
 */
function parseTitles(text: string): { title: string; titleOnly: boolean } {
  let title = "";
  let body = false;
  for (const raw of text.trim().split("\n")) {
    const line = raw.trim();
    if (line === "") {
      continue;
    }
    const found = headerTitle(line);
    if (found === null) {
      body = true;
    } else {
      title = found;
    }
  }
  return { title, titleOnly: title !== "" && !body };
}

/**
 * The details after a thought's verb, or null for none.
 *
 * - Unknown or under 500ms: "briefly", or none for title-only thinking.
 * - Title-only thinking from 500ms up to a second shows one decimal
 *   truncated to tenths (`0.6s`, never a rounded-up `1.0s`).
 * - Otherwise whole seconds rounded to the nearest (`4s`, `300s`).
 */
function thoughtDetails(durationMs: number | null, titleOnly: boolean): string | null {
  if (durationMs === null || durationMs < 500) {
    return titleOnly ? null : "briefly";
  }
  if (titleOnly && durationMs < 1000) {
    return `${(Math.floor(durationMs / 100) / 10).toFixed(1)}s`;
  }
  return `${Math.round(durationMs / 1000)}s`;
}

/** The summed duration of some thoughts, or null when none has a known one. */
function sumDurations(parts: readonly ThoughtInput[]): number | null {
  let total: number | null = null;
  for (const part of parts) {
    if (part.durationMs !== null) {
      total = (total ?? 0) + part.durationMs;
    }
  }
  return total;
}

/**
 * The header of one thought or several consecutive ones. Streaming
 * thinking is "Thinking" with no details. Settled thinking is "Thought"
 * with details - "briefly" when the duration is unknown or under 500ms,
 * else whole seconds ("4s", "300s": never "for", never minutes). When
 * every non-blank line is a title, the last title replaces "Thought" and
 * its details follow `thoughtDetails`' title-only rules. The duration is
 * the sum of the thoughts' durations.
 */
export function thoughtLabel(parts: readonly ThoughtInput[]): ThoughtLabel {
  if (parts.some((part) => part.streaming)) {
    return { action: "Thinking", details: null };
  }
  const { title, titleOnly } = parseTitles(parts.map((part) => part.text).join("\n"));
  return {
    action: titleOnly ? title : "Thought",
    details: thoughtDetails(sumDurations(parts), titleOnly),
  };
}

// --- Building ------------------------------------------------------------------------

/** One item with its index in the session's items. */
interface Entry {
  readonly item: TranscriptItem;
  readonly index: number;
}

/** A tool call as it is gathered, before loading and labels are known. */
interface ToolDraft {
  readonly key: string;
  readonly call: ToolCallRow;
  readonly family: ToolKind;
  result: string | null;
}

type StepDraft =
  | { readonly step: "thought"; readonly key: string; readonly item: ReasoningItem }
  | { readonly step: "tool"; readonly draft: ToolDraft };

type RowDraft =
  | { readonly row: "human"; readonly key: string; readonly text: string }
  | { readonly row: "markdown"; readonly key: string; readonly text: string; readonly streaming: boolean }
  | { readonly row: "flow"; readonly steps: readonly StepDraft[] }
  | { readonly row: "ask"; readonly draft: ToolDraft };

/** Splits items into turns: a user item starts each, and the items before the first form their own. */
function splitTurns(items: readonly TranscriptItem[]): Entry[][] {
  const turns: Entry[][] = [];
  let current: Entry[] | null = null;
  for (const [index, item] of items.entries()) {
    if (item.kind === "user" || current === null) {
      current = [];
      turns.push(current);
    }
    current.push({ item, index });
  }
  return turns;
}

/** Gathers one turn's entries into row drafts, attaching results to calls. */
function draftTurn(turn: number, entries: readonly Entry[]): RowDraft[] {
  const drafts: RowDraft[] = [];
  let open: StepDraft[] = [];
  const used = new Set<string>();
  // The latest call per id still waiting for its result.
  const waiting = new Map<string, ToolDraft>();

  const flush = (): void => {
    if (open.length > 0) {
      drafts.push({ row: "flow", steps: open });
      open = [];
    }
  };
  // A key not yet used in this turn: a recycled id or reply id keeps its
  // first occurrence's plain key and gives the later ones a suffix.
  const claim = (key: string, suffix: string): string => {
    const unique = used.has(key) ? `${key}:${suffix}` : key;
    used.add(unique);
    return unique;
  };

  for (const { item, index } of entries) {
    switch (item.kind) {
      case "user": {
        drafts.push({ row: "human", key: `h:${index}`, text: item.text });
        break;
      }
      case "reply": {
        // Nothing to show for blank text: no row, and the group stays open.
        if (item.text.trim() === "") {
          break;
        }
        flush();
        drafts.push({
          row: "markdown",
          key: claim(item.reply === null ? `r:i${index}` : `r:${item.reply}`, `${index}`),
          text: item.text,
          streaming: item.pending,
        });
        break;
      }
      case "reasoning": {
        // A settled thought with no text has nothing to open; a pending
        // one is still "Thinking" and shows.
        if (!item.pending && item.text.trim() === "") {
          break;
        }
        open.push({
          step: "thought",
          key: claim(item.reply === null ? `t:i${index}` : `t:${item.reply}`, `${index}`),
          item,
        });
        break;
      }
      case "tool-call": {
        item.calls.forEach((call, position) => {
          const slot = `${index}.${position}`;
          const draft: ToolDraft = {
            key: claim(call.id === "" ? `c:${turn}:${slot}` : `c:${turn}:${call.id}`, slot),
            call,
            family: toolKind(call.tool),
            result: null,
          };
          if (call.id !== "") {
            waiting.set(call.id, draft);
          }
          if (draft.family === "ask") {
            // An ask stands alone, after the group it interrupts.
            flush();
            drafts.push({ row: "ask", draft });
          } else {
            open.push({ step: "tool", draft });
          }
        });
        break;
      }
      case "tool-result": {
        const id = item.toolCallId;
        const draft = id === null || id === "" ? undefined : waiting.get(id);
        if (id !== null && draft !== undefined) {
          draft.result = item.text;
          waiting.delete(id);
        }
        break;
      }
      case "error": {
        flush();
        break;
      }
    }
  }
  flush();
  return drafts;
}

function toolStep(draft: ToolDraft, state: TurnState): ToolStep {
  const loading = isToolLoading(draft.family, draft.result !== null, state);
  return {
    step: "tool",
    key: draft.key,
    call: draft.call,
    toolKind: draft.family,
    result: draft.result,
    loading,
    label: toolLabel(draft.call, loading),
  };
}

function thoughtStep(key: string, item: ReasoningItem): ThoughtStep {
  const part: ThoughtInput = {
    text: item.text,
    // Streaming follows the clock, not `pending`: a thought whose round
    // already streamed text has ended while its durable event is in flight.
    streaming: item.pending && item.endedAt === null,
    durationMs:
      item.startedAt !== null && item.endedAt !== null
        ? Math.max(0, item.endedAt - item.startedAt)
        : null,
  };
  return {
    step: "thought",
    key,
    ...part,
    label: thoughtLabel([part]),
  };
}

function thoughtRow(thoughts: readonly ThoughtStep[]): ThoughtRow {
  return {
    kind: "thought",
    key: thoughts[0]?.key ?? "",
    thoughts,
    text: thoughts.map((thought) => thought.text).join("\n\n"),
    streaming: thoughts.some((thought) => thought.streaming),
    durationMs: sumDurations(thoughts),
    label: thoughtLabel(thoughts),
  };
}

/** Classifies drafts into rows once every result has attached. */
function finishTurn(
  drafts: readonly RowDraft[],
  state: TurnState,
): TranscriptRow[] {
  const rows: TranscriptRow[] = [];
  drafts.forEach((draft, position) => {
    switch (draft.row) {
      case "human":
        rows.push({ kind: "human", key: draft.key, text: draft.text });
        return;
      case "markdown":
        rows.push({ kind: "markdown", key: draft.key, text: draft.text, streaming: draft.streaming });
        return;
      case "ask":
        rows.push({ kind: "tool", key: draft.draft.key, step: toolStep(draft.draft, state) });
        return;
      case "flow": {
        const steps: GroupStep[] = draft.steps.map((step) =>
          step.step === "thought" ? thoughtStep(step.key, step.item) : toolStep(step.draft, state),
        );
        const tools = steps.filter((step): step is ToolStep => step.step === "tool");
        const thoughts = steps.filter((step): step is ThoughtStep => step.step === "thought");
        if (tools.length === 0) {
          rows.push(thoughtRow(thoughts));
        } else if (tools.length === 1 && thoughts.length === 0) {
          const only = tools[0];
          if (only !== undefined) {
            rows.push({ kind: "tool", key: only.key, step: only });
          }
        } else {
          const active = state.last && state.generating && position === drafts.length - 1;
          rows.push({
            kind: "group",
            key: steps[0]?.key ?? "",
            steps,
            summary: groupSummary(
              {
                searches: tools.filter((step) => step.toolKind === "search").length,
                fetches: tools.filter((step) => step.toolKind === "fetch").length,
                tools: tools.filter((step) => step.toolKind === "other").length,
              },
              active,
            ),
            active,
          });
        }
        return;
      }
    }
  });
  return rows;
}

// --- Tail status ------------------------------------------------------------------------

function makeTail(kind: TailKind, action: string, inGroup: boolean, rest: Partial<TailStatus> = {}): TailStatus {
  return {
    kind,
    action,
    details: "",
    callName: null,
    inGroup,
    ...rest,
  };
}

/**
 * The status line under the last turn. None when nothing is generating
 * or the last row is streaming assistant text (the text is the status).
 * Reconnecting while the socket is down. Otherwise the newest activity
 * the rows don't already show: a running tool inside a group gives its
 * loading verb and details, streaming thinking inside a group gives
 * "Thinking" - and a standalone tool row or a thought row already
 * shimmers on its own, so the tail stays out of its way. With no
 * activity at all it plans.
 */
function tailStatus(
  turn: TranscriptTurn | undefined,
  generating: boolean,
  reconnecting: boolean,
): TailStatus | null {
  if (!generating || turn === undefined) {
    return null;
  }
  const rows = turn.rows;
  const lastRow = rows[rows.length - 1];
  if (lastRow?.kind === "markdown" && lastRow.streaming) {
    return null;
  }
  const inGroup = lastRow?.kind === "group";
  if (reconnecting) {
    return makeTail("reconnecting", "Reconnecting...", inGroup);
  }
  for (let index = rows.length - 1; index >= 0; index--) {
    const row = rows[index];
    if (row === undefined) {
      continue;
    }
    if (row.kind === "thought" && row.streaming) {
      return null;
    }
    if (row.kind === "tool" && row.step.loading) {
      return null;
    }
    if (row.kind === "group") {
      for (let at = row.steps.length - 1; at >= 0; at--) {
        const step = row.steps[at];
        if (step?.step === "tool" && step.loading) {
          return makeTail("tool", step.label.action, inGroup, {
            details: step.label.details,
            callName: step.label.callName,
          });
        }
        if (step?.step === "thought" && step.streaming) {
          return makeTail("thinking", "Thinking", inGroup);
        }
      }
    }
  }
  return makeTail("planning", "Planning next moves", inGroup);
}

/**
 * Turns the session's items into turns of keyed rows and a tail status.
 * Pure: the items are never mutated, and the same inputs give the same
 * model.
 */
export function buildTranscript(
  items: readonly TranscriptItem[],
  generating: boolean,
  reconnecting: boolean,
): TranscriptModel {
  const split = splitTurns(items);
  const turns = split.map((entries, number): TranscriptTurn => {
    const first = entries[0];
    const state: TurnState = { last: number === split.length - 1, generating };
    return {
      key: first?.item.kind === "user" ? `u:${first.index}` : "u:start",
      rows: finishTurn(draftTurn(number, entries), state),
    };
  });
  return { turns, tail: tailStatus(turns[turns.length - 1], generating, reconnecting) };
}
