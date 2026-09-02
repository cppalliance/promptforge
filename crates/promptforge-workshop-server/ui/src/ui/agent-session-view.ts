// The agent-session view: paints the service's transcript into a feed
// and pins the chat input to the pending input wait. The feed repaints
// by a prefix diff over item identity - the service replaces item
// objects when they change, so the first non-identical index marks where
// the repaint starts, and everything before it (the settled history) is
// never rebuilt. Every content string is untrusted model-, tool-, or
// user-authored data: reply and reasoning markdown renders through
// renderMarkdown, whose DOMPurify pass is the last step before the DOM;
// user text, tool output, and errors land through textContent.
//
// Dictation mounts on the same input: a push-to-talk mic beside the
// send button drives stt.ts, which splices the transcript into the box
// at the cursor. The mic stays visible and clickable whatever the state,
// so a click while blocked names the blocker on the status bar (a probe
// still in flight, a failed probe, no GPU, no provisioned speech models,
// or no wait pinned) instead of the control silently disappearing. A take follows
// the wait it dictates into: when the pinned wait dies - spent by a send,
// cancelled by the server, or reset by a new session - the live take is
// discarded, because a take that cannot be sent is a trap.

import "./agent-session.css";

import { Disposable } from "../base/lifecycle";
import type {
  AgentSessionService,
  ToolCallItem,
  TranscriptItem,
} from "../services/agent-session";
import { renderMarkdown } from "./markdown-render";
import { PromptInput } from "./prompt-input";
import { ToolCallCard } from "./tool-call-card";
import {
  setupStt,
  sttCapability,
  type SttCapability,
  type SttHandle,
  type SttStatus,
} from "./stt";
import { ICON_MIC } from "./workshop/icons";

/** One painted feed row, kept for the identity diff. */
interface RenderedRow {
  readonly item: TranscriptItem;
  readonly row: HTMLLIElement;
  /**
   * The row's tool card, when it paints one. A landing tool result
   * appends a new item rather than replacing the call item, so the
   * card's row survives the prefix diff and each repaint re-drives the
   * card's running state.
   */
  readonly card?: ToolCallCard;
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

/** The ids of every tool result in the transcript, for matching calls to outcomes. */
function toolResultIds(items: readonly TranscriptItem[]): ReadonlySet<string> {
  const ids = new Set<string>();
  for (const item of items) {
    if (item.kind === "tool-result" && item.toolCallId !== null && item.toolCallId !== "") {
      ids.add(item.toolCallId);
    }
  }
  return ids;
}

/**
 * True while a tool-call batch awaits its outcome: a batch runs until a
 * tool-result whose toolCallId matches one of its calls lands. Calls
 * without ids (an entry parsed with no string id, or an unparsed batch)
 * can never match, so they have nothing to await.
 */
function isToolCallRunning(item: ToolCallItem, resultIds: ReadonlySet<string>): boolean {
  let trackable = false;
  for (const call of item.calls) {
    if (call.id === "") {
      continue;
    }
    if (resultIds.has(call.id)) {
      return false;
    }
    trackable = true;
  }
  return trackable;
}

/** One rendered transcript item: its feed row plus its live tool card, when any. */
interface PaintedItem {
  readonly row: HTMLLIElement;
  readonly card?: ToolCallCard;
}

/** Renders one transcript item as a feed row. */
function renderItem(item: TranscriptItem, resultIds: ReadonlySet<string>): PaintedItem {
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
      row.appendChild(renderMarkdown(item.text, { streaming: item.pending }));
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
      block.append(summary, renderMarkdown(item.text, { streaming: item.pending }));
      row.appendChild(block);
      break;
    }
    case "tool-call": {
      row.appendChild(metaLine(item.model === null ? "Tool call" : `Tool call (${item.model})`));
      const card = new ToolCallCard(item, { running: isToolCallRunning(item, resultIds) });
      row.appendChild(card.element);
      return { row, card };
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
  return { row };
}

/**
 * The session surface: the transcript feed over the input bar. The
 * input enables only while a wait is pinned; submitting answers the wait
 * through the service and clears the box on a successful send. The
 * status sink receives dictation's local messages and REC badge state.
 */
export class AgentSessionView extends Disposable {
  readonly element: HTMLElement;
  /**
   * The prompt box under the feed. Exposed so tests can drive content
   * and selection - the DOM alone sets neither on a ProseMirror editor.
   */
  readonly promptInput: PromptInput;
  private readonly feed: HTMLOListElement;
  private readonly mic: HTMLButtonElement;
  private readonly send: HTMLButtonElement;
  private readonly stt: SttHandle;
  private rendered: RenderedRow[] = [];
  /** The capability probe's answer; undefined while it is in flight. */
  private capability: SttCapability | null | undefined;

  constructor(
    private readonly service: AgentSessionService,
    status: SttStatus,
  ) {
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

    const bar = document.createElement("div");
    bar.className = "agent-session__bar";
    this.mic = document.createElement("button");
    this.mic.type = "button";
    this.mic.className = "agent-session__mic stt-mic";
    this.mic.title = "Push to talk";
    this.mic.setAttribute("aria-label", "Push to talk");
    this.mic.setAttribute("aria-pressed", "false");
    // A static lucide string, not data: the only markup this view writes.
    this.mic.innerHTML = ICON_MIC;
    this.send = document.createElement("button");
    this.send.type = "button";
    this.send.className = "agent-session__send";
    this.send.textContent = "Send";
    this.send.addEventListener("click", () => this.submit());
    const promptInput = new PromptInput({
      // A function, not a fixed string: the placeholder names the gate's
      // state, and the Placeholder decoration re-evaluates it on every
      // state update (setEditable triggers one).
      placeholder: () =>
        this.service.pendingInputToken === null
          ? "The agent is working; the input opens when it asks"
          : "Message the agent",
      ariaLabel: "Message",
      onSubmit: () => this.submit(),
    });
    bar.append(promptInput.element, this.mic, this.send);
    this.element.append(this.feed, bar);

    // Element-owned listeners die with the elements; only service
    // subscriptions need the lifecycle.
    this._register(this.service.onDidChangeTranscript(() => this.renderFeed()));
    this._register(
      this.service.onDidChangePendingInput((token) => {
        if (token === null) {
          this.stt.discardIfRecording();
        }
        this.renderInputState();
      }),
    );

    // The dictation control over the mic and input. Registered before the
    // prompt input so disposal discards a live take while the editor
    // still stands. The blocker names the first reason a take cannot
    // start, capability before the wait. The probe resolves after mount;
    // a click that beats it is refused, because a server with no engine
    // still accepts /stt and answers an empty final, so an unchecked
    // take would record for nothing.
    this.stt = this._register(
      setupStt({ mic: this.mic, input: promptInput }, status, () => {
        if (this.capability === undefined) {
          return "Dictation is still checking what this server can do; try again in a moment.";
        }
        if (this.capability === null) {
          return "Dictation is unavailable: the server's capability probe failed.";
        }
        if (!this.capability.gpu) {
          return "Dictation needs a GPU this server doesn't have.";
        }
        if (!this.capability.engine) {
          return "No speech models are provisioned in the active profile.";
        }
        if (this.service.pendingInputToken === null) {
          return "The agent isn't asking for input; the mic opens when it does.";
        }
        return null;
      }),
    );
    this.promptInput = this._register(promptInput);

    this.renderFeed();
    this.renderInputState();

    void sttCapability().then((answer) => {
      this.capability = answer;
    });
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
    const resultIds = toolResultIds(items);
    // A result that just landed leaves its call item's identity alone,
    // so surviving cards are re-driven here; setRunning is a no-op on an
    // unchanged state, so a card the operator opened is never slammed.
    for (const painted of this.rendered) {
      if (painted.card !== undefined && painted.item.kind === "tool-call") {
        painted.card.setRunning(isToolCallRunning(painted.item, resultIds));
      }
    }
    for (const item of items.slice(first)) {
      const painted = renderItem(item, resultIds);
      this.feed.appendChild(painted.row);
      this.rendered.push({ item, row: painted.row, card: painted.card });
    }
    this.feed.scrollTop = this.feed.scrollHeight;
  }

  /** Pins the input to the pending wait: editable only while one is open. */
  private renderInputState(): void {
    const pinned = this.service.pendingInputToken !== null;
    this.promptInput.setEditable(pinned);
    this.send.disabled = !pinned;
  }

  /**
   * Answers the pending wait with the box's text, byte-exact - never
   * trimmed, because the wire contract is what the operator typed. An
   * empty box sends nothing; a failed send keeps the text for the retry.
   * A send ends a live take: what the operator sees in the box, interim
   * transcript included, is what goes; the take's polished final is
   * discarded rather than landing in a box that already sent.
   */
  private submit(): void {
    const text = this.promptInput.getText();
    if (text === "" || this.service.pendingInputToken === null) {
      return;
    }
    // Read before discarding: the discard restores the box to its
    // pre-take text, and the send carries what was showing.
    this.stt.discardIfRecording();
    if (this.service.respond(text)) {
      this.promptInput.clear();
    }
  }
}
