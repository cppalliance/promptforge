// Collapsible card for one tool-call batch: a <details>/<summary> header
// (status dot, tool name, call-count badge) over a body of Shiki-
// highlighted argument JSON and, once it lands, the matched tool result
// in a scrollable <pre>. The wire protocol has no pending flag for tool
// calls, so the caller computes `running` - a call runs until a
// tool-result with a matching toolCallId appears later in the
// transcript - and the card auto-opens while running and auto-collapses
// when running flips false. Args HTML comes from highlightCode, which
// escapes its input on both the Shiki and the plain-fallback path; the
// result text is untrusted tool output and lands through textContent.

import "./tool-call-card.css";

import type { ToolCallItem } from "../services/agent-session";
import { highlightCode } from "./markdown-render";

/** Construction options for {@link ToolCallCard}. */
export interface ToolCallCardOptions {
  /** True while the batch awaits its result; the card opens and pulses. */
  readonly running: boolean;
  /** The matched tool result text, when one has already landed. */
  readonly result?: string | null;
}

/** Picks the header name for a batch: the shared call name, or a generic label. */
function batchName(item: ToolCallItem): string {
  const first = item.calls[0];
  if (first === undefined || first.name === "") {
    return "Tool call";
  }
  for (const call of item.calls) {
    if (call.name !== first.name) {
      return "Tool calls";
    }
  }
  return first.name;
}

/**
 * One tool-call batch as a collapsible card. Pure view: no listeners and
 * no timers (the disclosure toggle is native and the running pulse is
 * CSS), so there is nothing to dispose.
 */
export class ToolCallCard {
  /** The card's root, ready to append to a feed row. */
  readonly element: HTMLDetailsElement;

  private readonly statusText: HTMLSpanElement;
  private readonly resultPre: HTMLPreElement;
  private isRunning: boolean;

  constructor(item: ToolCallItem, options: ToolCallCardOptions) {
    this.element = document.createElement("details");
    this.element.className = "tool-call-card";

    const summary = document.createElement("summary");
    summary.className = "tool-call-card__summary";

    const status = document.createElement("span");
    status.className = "tool-call-card__status";
    status.setAttribute("aria-hidden", "true");

    this.statusText = document.createElement("span");
    this.statusText.className = "tool-call-card__sr";

    const name = document.createElement("span");
    name.className = "tool-call-card__name";
    name.textContent = batchName(item);

    summary.append(status, this.statusText, name);
    if (item.calls.length > 0) {
      const count = document.createElement("span");
      count.className = "tool-call-card__count";
      count.textContent = String(item.calls.length);
      summary.appendChild(count);
    }

    const body = document.createElement("div");
    body.className = "tool-call-card__body";
    if (item.calls.length === 0) {
      // The batch JSON did not parse; the raw content still renders.
      const raw = document.createElement("pre");
      raw.className = "tool-call-card__raw";
      raw.textContent = item.text;
      body.appendChild(raw);
    } else {
      for (const call of item.calls) {
        const block = document.createElement("div");
        block.className = "tool-call-card__call";
        if (item.calls.length > 1) {
          const label = document.createElement("p");
          label.className = "tool-call-card__call-name";
          label.textContent = call.name === "" ? "Tool call" : call.name;
          block.appendChild(label);
        }
        if (call.args !== "") {
          const args = document.createElement("div");
          args.className = "tool-call-card__args";
          // highlightCode escapes on both its paths; its HTML is safe.
          args.innerHTML = highlightCode(call.args, "json");
          block.appendChild(args);
        }
        body.appendChild(block);
      }
    }

    this.resultPre = document.createElement("pre");
    this.resultPre.className = "tool-call-card__result";
    this.resultPre.hidden = true;
    body.appendChild(this.resultPre);

    this.element.append(summary, body);

    this.isRunning = options.running;
    if (options.running) {
      this.element.classList.add("tool-call-card--running");
      this.element.open = true;
    }
    this.statusText.textContent = options.running ? "Running" : "Completed";
    if (options.result != null) {
      this.setResult(options.result);
    }
  }

  /**
   * Drives the running affordance: opens the card while the batch runs
   * and collapses it when the result lands. A no-op when the state is
   * unchanged, so a repaint that recomputes the same settled state never
   * slams a card the operator opened.
   */
  setRunning(running: boolean): void {
    if (running === this.isRunning) {
      return;
    }
    this.isRunning = running;
    this.element.classList.toggle("tool-call-card--running", running);
    this.element.open = running;
    this.statusText.textContent = running ? "Running" : "Completed";
  }

  /** Shows the matched tool result, or hides the block again on null. */
  setResult(result: string | null): void {
    if (result === null) {
      this.resultPre.textContent = "";
      this.resultPre.hidden = true;
      return;
    }
    this.resultPre.textContent = result;
    this.resultPre.hidden = false;
  }
}
