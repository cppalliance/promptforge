// A tool call as the transcript shows it: one line, "Ran read in fs" or
// "Searched web <query>", used both as a standalone tool row and as a
// step inside a group. The verb and details come from the model's tool
// label; the line adds the behavior.
//
// - Search and fetch lines are not expandable. They open a hover card
//   (web-hover-card.ts) listing the result titles and URLs, disabled
//   while the call is loading.
// - An ask line is plain text; it waits on the operator, not on a result.
// - Any other tool expands into one block - its arguments as indented
//   JSON, a blank line, then its result - when it has either.
//
// While the call is loading only the verb shimmers; there are no icons,
// dots, or badges. Arguments and results are untrusted tool data and land
// through textContent. Open state lives in the line's Collapsible, so it
// survives every `update`.

import "./tool-line.css";

import type { IDisposable } from "@workshop/platform/lifecycle";
import { Collapsible } from "./collapsible";
import type { ToolStep } from "./transcript-model";
import { WebHoverCard, webCardEntries } from "./web-hover-card";

/** The arguments as indented JSON, the raw text when they don't parse, or "" for none. */
function prettyArguments(args: string): string {
  if (args === "") {
    return "";
  }
  try {
    return JSON.stringify(JSON.parse(args) as unknown, null, 2);
  } catch {
    return args;
  }
}

/** The generic block's text: the arguments, a blank line, then the result. */
export function toolBlockText(step: ToolStep): string {
  const parts: string[] = [];
  const args = prettyArguments(step.call.args);
  if (args !== "") {
    parts.push(args);
  }
  if (step.result !== null && step.result !== "") {
    parts.push(step.result);
  }
  return parts.join("\n\n");
}

/** One tool line. */
export class ToolLine implements IDisposable {
  readonly kind = "tool";
  readonly element: HTMLElement;

  private readonly collapsible: Collapsible;
  private readonly pre: HTMLPreElement;
  private readonly card: WebHoverCard;
  private text = "";
  private label = "";

  constructor() {
    this.collapsible = new Collapsible();
    this.element = this.collapsible.element;
    this.element.classList.add("ws-tool-line");

    const block = document.createElement("div");
    block.className = "ws-tool-block";
    this.pre = document.createElement("pre");
    this.pre.className = "ws-tool-block__pre";
    block.appendChild(this.pre);
    this.collapsible.body.appendChild(block);

    // The card hangs off the header line, not the whole element, so an
    // opened block never keeps it alive.
    this.card = new WebHoverCard(this.collapsible.header);
  }

  /** Paints the step, leaving the open state and the card's timers alone. */
  update(step: ToolStep): void {
    this.collapsible.setHeader({
      action: step.label.action,
      details: step.label.details,
      callName: step.label.callName,
      shimmer: step.loading ? "tool" : null,
    });
    this.element.dataset["toolKind"] = step.toolKind;
    this.element.dataset["loading"] = String(step.loading);
    this.label = `${step.label.action} ${step.label.details}`.trim();

    if (step.toolKind === "other") {
      const text = toolBlockText(step);
      if (text !== this.text) {
        this.text = text;
        this.pre.textContent = text;
      }
      this.collapsible.setExpandable(text !== "");
      this.card.setEntries(null);
      return;
    }

    this.text = "";
    this.pre.textContent = "";
    this.collapsible.setExpandable(false);
    if (step.toolKind === "search" || step.toolKind === "fetch") {
      this.card.setEntries(webCardEntries(step.toolKind, step.call.args, step.result));
      this.card.setDisabled(step.loading);
    } else {
      this.card.setEntries(null);
    }
  }

  /** The operator's own open or closed choice for the block, or null while they've made none. */
  get operatorChoice(): boolean | null {
    return this.collapsible.operatorChoice;
  }

  /** Takes over the operator's choice from the line this one replaces. */
  adoptChoice(choice: boolean | null): void {
    this.collapsible.adoptChoice(choice);
  }

  /** The line's visible text, for the transcript's Copy Message. */
  copyText(): string {
    return this.label;
  }

  dispose(): void {
    this.card.dispose();
  }
}
