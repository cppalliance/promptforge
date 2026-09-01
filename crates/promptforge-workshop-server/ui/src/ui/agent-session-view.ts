// The agent-session view: paints the service's transcript into a feed
// and pins the chat input to the pending input wait. The feed repaints
// by a prefix diff over item identity - the service replaces item
// objects when they change, so the first non-identical index marks where
// the repaint starts, and everything before it (the settled history) is
// never rebuilt. Every content string is untrusted model-, tool-, or
// user-authored data and lands through textContent, never markup.

import "./agent-session.css";

import { Disposable } from "../base/lifecycle";
import type { AgentSessionService, TranscriptItem } from "../services/agent-session";

/** One painted feed row, kept for the identity diff. */
interface RenderedRow {
  readonly item: TranscriptItem;
  readonly row: HTMLLIElement;
}

/** The muted origin line above a row's content. */
function metaLine(text: string): HTMLParagraphElement {
  const meta = document.createElement("p");
  meta.className = "agent-item__meta";
  meta.textContent = text;
  return meta;
}

/** The row's content paragraph, untrusted text as text. */
function textBlock(text: string): HTMLParagraphElement {
  const block = document.createElement("p");
  block.className = "agent-item__text";
  block.textContent = text;
  return block;
}

/** Renders one transcript item as a feed row. */
function renderItem(item: TranscriptItem): HTMLLIElement {
  const row = document.createElement("li");
  row.className = `agent-item agent-item--${item.kind}`;
  switch (item.kind) {
    case "user": {
      row.append(metaLine("You"), textBlock(item.text));
      break;
    }
    case "reply": {
      if (item.pending) {
        row.classList.add("agent-item--pending");
      }
      if (item.model !== null) {
        row.appendChild(metaLine(item.model));
      }
      row.appendChild(textBlock(item.text));
      break;
    }
    case "reasoning": {
      if (item.pending) {
        row.classList.add("agent-item--pending");
      }
      const block = document.createElement("details");
      block.className = "agent-item__reasoning";
      // Open while streaming so the thinking is watchable; the settled
      // block collapses out of the way of the reply that follows it.
      block.open = item.pending;
      const summary = document.createElement("summary");
      summary.textContent = item.model === null ? "Reasoning" : `Reasoning (${item.model})`;
      block.append(summary, textBlock(item.text));
      row.appendChild(block);
      break;
    }
    case "tool-call": {
      row.appendChild(metaLine(item.model === null ? "Tool call" : `Tool call (${item.model})`));
      if (item.calls.length === 0) {
        // The batch JSON did not parse; the raw content still renders.
        row.appendChild(textBlock(item.text));
        break;
      }
      const calls = document.createElement("ul");
      calls.className = "agent-item__calls";
      for (const call of item.calls) {
        const entry = document.createElement("li");
        entry.className = "agent-item__call";
        const name = document.createElement("code");
        name.className = "agent-item__call-name";
        name.textContent = call.name;
        entry.appendChild(name);
        if (call.args !== "") {
          const args = document.createElement("code");
          args.className = "agent-item__call-args";
          args.textContent = call.args;
          entry.appendChild(args);
        }
        calls.appendChild(entry);
      }
      row.appendChild(calls);
      break;
    }
    case "tool-result": {
      row.appendChild(
        metaLine(item.toolCallId === null ? "Tool result" : `Tool result (${item.toolCallId})`),
      );
      const output = document.createElement("pre");
      output.className = "agent-item__output";
      output.textContent = item.text;
      row.appendChild(output);
      break;
    }
    case "error": {
      const message = document.createElement("p");
      message.className = "agent-item__text";
      const label = document.createElement("strong");
      // A visible label, so the failure never signals by color alone.
      label.textContent = "Error: ";
      message.append(label, item.message);
      row.appendChild(message);
      break;
    }
  }
  return row;
}

/**
 * The session surface: the transcript feed over the input form. The
 * input enables only while a wait is pinned; submitting answers the wait
 * through the service and clears the box on a successful send.
 */
export class AgentSessionView extends Disposable {
  readonly element: HTMLElement;
  private readonly feed: HTMLOListElement;
  private readonly input: HTMLTextAreaElement;
  private readonly send: HTMLButtonElement;
  private rendered: RenderedRow[] = [];

  constructor(private readonly service: AgentSessionService) {
    super();
    this.element = document.createElement("section");
    this.element.className = "agent-session";
    this.element.setAttribute("aria-label", "Agent session");

    this.feed = document.createElement("ol");
    this.feed.className = "agent-session__feed";
    // A live list, not role="log": the role would replace the list
    // semantics, and the property alone announces appended rows.
    this.feed.setAttribute("aria-live", "polite");
    this.feed.setAttribute("aria-atomic", "false");

    const form = document.createElement("form");
    form.className = "agent-session__form";
    this.input = document.createElement("textarea");
    this.input.className = "agent-session__input";
    this.input.rows = 1;
    this.input.setAttribute("aria-label", "Message");
    this.send = document.createElement("button");
    this.send.type = "submit";
    this.send.className = "agent-session__send";
    this.send.textContent = "Send";
    form.append(this.input, this.send);
    this.element.append(this.feed, form);

    // Element-owned listeners die with the elements; only service
    // subscriptions need the lifecycle.
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      this.submit();
    });
    this.input.addEventListener("keydown", (event) => {
      // An Enter that commits an IME composition is not a send: without
      // the isComposing guard the box would submit half-composed text.
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        this.submit();
      }
    });

    this._register(this.service.onDidChangeTranscript(() => this.renderFeed()));
    this._register(this.service.onDidChangePendingInput(() => this.renderInputState()));
    this.renderFeed();
    this.renderInputState();
  }

  /**
   * Repaints the feed from the first index whose item is not the very
   * object painted there: everything past it is removed and re-rendered,
   * everything before it stands. Streaming touches only the tail, so the
   * settled history never rebuilds (and is never re-announced).
   */
  private renderFeed(): void {
    const items = this.service.items;
    let first = 0;
    while (first < this.rendered.length && first < items.length) {
      const painted: RenderedRow | undefined = this.rendered[first];
      if (painted === undefined || painted.item !== items[first]) {
        break;
      }
      first++;
    }
    for (const stale of this.rendered.splice(first)) {
      stale.row.remove();
    }
    for (const item of items.slice(first)) {
      const row = renderItem(item);
      this.feed.appendChild(row);
      this.rendered.push({ item, row });
    }
    this.feed.scrollTop = this.feed.scrollHeight;
  }

  /** Pins the input to the pending wait: enabled only while one is open. */
  private renderInputState(): void {
    const pinned = this.service.pendingInputToken !== null;
    this.input.disabled = !pinned;
    this.send.disabled = !pinned;
    this.input.placeholder = pinned
      ? "Message the agent"
      : "The agent is working; the input opens when it asks";
  }

  /**
   * Answers the pending wait with the box's text, byte-exact - never
   * trimmed, because the wire contract is what the operator typed. An
   * empty box sends nothing; a failed send keeps the text for the retry.
   */
  private submit(): void {
    const text = this.input.value;
    if (text === "" || this.service.pendingInputToken === null) {
      return;
    }
    if (this.service.respond(text)) {
      this.input.value = "";
    }
  }
}
