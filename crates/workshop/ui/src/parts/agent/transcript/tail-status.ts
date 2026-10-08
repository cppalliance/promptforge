// The tail status: the shimmering line under the last row while the agent
// works - "Planning next moves", "Thinking", a running tool's verb, or
// "Reconnecting...". The model decides which (transcript-model.ts); this
// paints it. The status has no Cancel action: stopping a turn belongs to
// the composer's action button.
//
// The status renders at the end of the last turn, or inside the open
// group that is the active tail (the view moves the element). The verb
// shimmers in the tone for its kind; details are untrusted model-era text
// and land through textContent.

import "./transcript.css";

import type { IDisposable } from "@workshop/platform/lifecycle";
import { applyShimmer } from "./collapsible";
import type { TailStatus } from "./transcript-model";

/** The tail status line. */
export class TailView implements IDisposable {
  readonly element: HTMLDivElement;

  private readonly action: HTMLSpanElement;
  private readonly details: HTMLSpanElement;
  private detailsKey: string | null = null;

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "ws-tail";
    this.action = document.createElement("span");
    this.action.className = "ws-tail__action";
    this.details = document.createElement("span");
    this.details.className = "ws-tail__details";
    this.element.append(this.action, this.details);
  }

  /** Paints the status, touching the DOM only where the text changed. */
  update(tail: TailStatus): void {
    this.element.dataset["kind"] = tail.kind;
    if (this.action.textContent !== tail.action) {
      this.action.textContent = tail.action;
    }
    const key = `${tail.callName ?? ""}\u0000${tail.details}`;
    if (key !== this.detailsKey) {
      this.detailsKey = key;
      this.details.replaceChildren();
      if (tail.callName !== null && tail.callName !== "" && tail.details.startsWith(tail.callName)) {
        const call = document.createElement("span");
        call.className = "ws-tail__call";
        call.textContent = tail.callName;
        this.details.append(call, tail.details.slice(tail.callName.length));
      } else {
        this.details.textContent = tail.details;
      }
      this.details.hidden = tail.details === "";
    }
    applyShimmer(this.action, tail.kind === "tool" ? "tool" : "status");
  }

  dispose(): void {
    applyShimmer(this.action, null);
  }
}
