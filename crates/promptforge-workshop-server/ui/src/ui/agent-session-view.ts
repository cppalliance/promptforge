// The agent-session view: paints the service's transcript into a feed
// and pins the chat input to the pending input wait. The feed repaints
// by a prefix diff over item identity - the service replaces item
// objects when they change, so the first non-identical index marks where
// the repaint starts, and everything before it (the settled history) is
// never rebuilt. Every content string is untrusted model-, tool-, or
// user-authored data and lands through textContent, never markup.
//
// Voice dictation mounts on the same input: a push-to-talk mic beside the
// send button drives voice.ts, which splices the transcript into the box
// at the cursor. The mic stays visible and clickable whatever the state,
// so a click while blocked names the blocker on the status bar (a probe
// still in flight, a failed probe, no GPU, no provisioned speech models,
// or no wait pinned) instead of the control silently disappearing. A take follows
// the wait it dictates into: when the pinned wait dies - spent by a send,
// cancelled by the server, or reset by a new session - the live take is
// discarded, because a take that cannot be sent is a trap.

import "./agent-session.css";

import { Disposable } from "../base/lifecycle";
import type { AgentSessionService, TranscriptItem } from "../services/agent-session";
import {
  setupVoice,
  voiceCapability,
  type VoiceCapability,
  type VoiceHandle,
  type VoiceStatus,
} from "./voice";
import { ICON_MIC } from "./workshop/icons";

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
 * through the service and clears the box on a successful send. The
 * status sink receives voice's local messages and REC badge state.
 */
export class AgentSessionView extends Disposable {
  readonly element: HTMLElement;
  private readonly feed: HTMLOListElement;
  private readonly input: HTMLTextAreaElement;
  private readonly mic: HTMLButtonElement;
  private readonly send: HTMLButtonElement;
  private readonly voice: VoiceHandle;
  private rendered: RenderedRow[] = [];
  /** The capability probe's answer; undefined while it is in flight. */
  private capability: VoiceCapability | null | undefined;

  constructor(
    private readonly service: AgentSessionService,
    status: VoiceStatus,
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

    const form = document.createElement("form");
    form.className = "agent-session__form";
    this.input = document.createElement("textarea");
    this.input.className = "agent-session__input";
    this.input.rows = 1;
    this.input.setAttribute("aria-label", "Message");
    this.mic = document.createElement("button");
    this.mic.type = "button";
    this.mic.className = "agent-session__mic voice-mic";
    this.mic.title = "Push to talk";
    this.mic.setAttribute("aria-label", "Push to talk");
    this.mic.setAttribute("aria-pressed", "false");
    // A static lucide string, not data: the only markup this view writes.
    this.mic.innerHTML = ICON_MIC;
    this.send = document.createElement("button");
    this.send.type = "submit";
    this.send.className = "agent-session__send";
    this.send.textContent = "Send";
    form.append(this.input, this.mic, this.send);
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
    // Discard before repaint: the take lifts the input's readOnly, and the
    // repaint then disables the box against the dead wait.
    this._register(
      this.service.onDidChangePendingInput((token) => {
        if (token === null) {
          this.voice.discardIfRecording();
        }
        this.renderInputState();
      }),
    );
    this.renderFeed();
    this.renderInputState();

    // The voice control over the mic and input; registered so disposing
    // the view unwires the mic and discards a live take. The blocker
    // names the first reason a take cannot start, capability before the
    // wait. The probe resolves after mount; a click that beats it is
    // refused, because a server with no engine still accepts /voice and
    // answers an empty final, so an unchecked take would record for
    // nothing.
    this.voice = this._register(
      setupVoice({ mic: this.mic, input: this.input }, status, () => {
        if (this.capability === undefined) {
          return "Voice dictation is still checking what this server can do; try again in a moment.";
        }
        if (this.capability === null) {
          return "Voice dictation is unavailable: the server's capability probe failed.";
        }
        if (!this.capability.gpu) {
          return "Voice dictation needs a GPU this server doesn't have.";
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
    void voiceCapability().then((answer) => {
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
   * A send ends a live take: what the operator sees in the box, interim
   * transcript included, is what goes; the take's polished final is
   * discarded rather than landing in a box that already sent.
   */
  private submit(): void {
    const text = this.input.value;
    if (text === "" || this.service.pendingInputToken === null) {
      return;
    }
    // Read before discarding: the discard restores the box to its
    // pre-take text, and the send carries what was showing.
    this.voice.discardIfRecording();
    this.input.value = this.service.respond(text) ? "" : text;
  }
}
