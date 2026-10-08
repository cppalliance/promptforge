// Tool labels for the agent transcript: the verb and details a tool call
// shows, the loading rule, and the grammar of a group's summary. Pure and
// DOM-free; the transcript model composes these into rows, and the view
// only paints what they return. The tool id and the call arguments are
// untrusted model-era data, so every parse here degrades to "no details"
// rather than throwing.

import type { ToolCallRow } from "../../../services/agent-session";

/** The families of tool the transcript words differently. */
export type ToolKind = "search" | "fetch" | "ask" | "other";

/** What a tool line reads: a verb, details after it, and the call name's span. */
export interface ToolLabel {
  /** The verb: "Searching web" while loading, "Searched web" once done. */
  readonly action: string;
  /** The text after the verb; "" when the call has none. */
  readonly details: string;
  /**
   * The call name when it leads `details` and renders in the verb's
   * color (a generic tool's `read in fs`), or null when `details` is
   * plain.
   */
  readonly callName: string | null;
}

/** How many calls of each family a group holds. */
export interface ToolCounts {
  readonly searches: number;
  readonly fetches: number;
  readonly tools: number;
}

/** A group's header: the verb and the parts that follow it. */
export interface GroupSummary {
  readonly action: string;
  readonly details: string;
}

/** The turn a tool call sits in, as the loading rule reads it. */
export interface TurnState {
  /** True for the transcript's last turn. */
  readonly last: boolean;
  /** True while the service is generating. */
  readonly generating: boolean;
}

/**
 * The family of a bound tool id (the wire's `namespace/name` string).
 * The ask tool is recognized by its name, because a Host installs the
 * user-input Plugin under any namespace; the web tools are fixed ids.
 * A null tool is generic.
 */
export function toolKind(tool: string | null): ToolKind {
  if (tool === "web/search") {
    return "search";
  }
  if (tool === "web/fetch") {
    return "fetch";
  }
  if (tool !== null && tool.slice(tool.lastIndexOf("/") + 1) === "ask") {
    return "ask";
  }
  return "other";
}

/**
 * Whether a call is loading. A call is loading while it has no result
 * and its turn is generating. An ask call is also loading while it waits
 * for the operator's answer, because `generating` is off during a wait;
 * that wait can only be in the last turn, so an ask left unanswered in an
 * earlier turn (a cancelled wait) is not loading.
 */
export function isToolLoading(kind: ToolKind, hasResult: boolean, turn: TurnState): boolean {
  if (hasResult) {
    return false;
  }
  if (kind === "ask") {
    return turn.last;
  }
  return turn.last && turn.generating;
}

/** One string argument of a call's compact-JSON arguments, or "". */
function stringArg(args: string, key: string): string {
  if (args === "") {
    return "";
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(args);
  } catch {
    return "";
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return "";
  }
  const value = (parsed as Record<string, unknown>)[key];
  return typeof value === "string" ? value : "";
}

/**
 * The verb and details of one tool call. `web/search` and `web/fetch`
 * read their query and url arguments; the ask tool has no details; any
 * other tool reads `<call name> in <tool id namespace>`, with the call
 * name flagged for the verb's color.
 */
export function toolLabel(call: ToolCallRow, loading: boolean): ToolLabel {
  switch (toolKind(call.tool)) {
    case "search":
      return {
        action: loading ? "Searching web" : "Searched web",
        details: stringArg(call.args, "query"),
        callName: null,
      };
    case "fetch":
      return {
        action: loading ? "Fetching page" : "Fetched page",
        details: stringArg(call.args, "url"),
        callName: null,
      };
    case "ask":
      return {
        action: loading ? "Asking questions" : "Asked questions",
        details: "",
        callName: null,
      };
    case "other": {
      const action = loading ? "Running" : "Ran";
      if (call.name === "") {
        return { action, details: call.tool ?? "", callName: null };
      }
      const namespace = call.tool === null ? "" : call.tool.split("/")[0] ?? "";
      return {
        action,
        details: namespace === "" ? call.name : `${call.name} in ${namespace}`,
        callName: call.name,
      };
    }
  }
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/**
 * A group's header. The verb is Exploring while the group is the last row
 * of a generating turn and Explored otherwise. The parts come in a fixed
 * order - searches, fetches, other tools - and are joined with ", ": there
 * is no "and". A family with no calls is left out.
 */
export function groupSummary(counts: ToolCounts, active: boolean): GroupSummary {
  const parts: string[] = [];
  if (counts.searches > 0) {
    parts.push(plural(counts.searches, "search", "searches"));
  }
  if (counts.fetches > 0) {
    parts.push(plural(counts.fetches, "fetch", "fetches"));
  }
  if (counts.tools > 0) {
    parts.push(plural(counts.tools, "tool", "tools"));
  }
  return { action: active ? "Exploring" : "Explored", details: parts.join(", ") };
}
