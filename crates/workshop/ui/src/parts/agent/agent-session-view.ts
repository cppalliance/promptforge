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
// The composer is the ChatBox component: this view is its host. It maps
// service state to the box's props (editable follows the pinned wait,
// the send action follows the wait and the model selection, the mic
// follows dictation's state) and routes the box's events back - `send`
// answers the wait, `mic-press` drives stt.ts, which splices the
// transcript into the box at the cursor. The mic stays visible and
// clickable whatever the state, so a click while blocked names the
// blocker on the status bar instead of the control silently
// disappearing. A take follows the wait it dictates into: when the
// pinned wait dies - spent by a send, cancelled by the server, or reset
// by a new session - the live take is discarded, because a take that
// cannot be sent is a trap.

import "./agent-session.css";

import { Disposable } from "../../base/lifecycle";
import type {
  AgentSessionService,
  ToolCallItem,
  TranscriptItem,
} from "../../services/agent-session";
import type { ModelService } from "../../services/model-service";
import { getServiceOrNull } from "../../services/service-registry";
import { SpeechCaptureService } from "../../services/speech-capture";
import { TEXT_CONTROL_SERVICE } from "../../services/text-control-service";
import { AgentToolbar } from "./agent-toolbar";
import { renderMarkdown } from "./markdown-render";
import { ChatBox } from "../chatbox/chat-box";
import type { ChatBoxEvent, ChatBoxProps } from "../chatbox/types";
import { ToolCallCard } from "./tool-call-card";
import {
  setupStt,
  type SttHandle,
  type SttStatus,
} from "../stt/stt";

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
  meta.className = "ws-agent-item__meta";
  meta.textContent = text;
  return meta;
}

/** The row's content paragraph, untrusted text as text. */
function textBlock(text: string): HTMLParagraphElement {
  const block = document.createElement("p");
  block.className = "ws-agent-item__text";
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
  row.className = `ws-agent-item ws-agent-item--${item.kind}`;
  switch (item.kind) {
    case "user": {
      row.append(metaLine("You"), textBlock(item.text));
      break;
    }
    case "reply": {
      if (item.pending) {
        row.classList.add("ws-agent-item--pending");
      }
      if (item.model !== null) {
        row.appendChild(metaLine(item.model));
      }
      row.appendChild(renderMarkdown(item.text, { streaming: item.pending }));
      break;
    }
    case "reasoning": {
      if (item.pending) {
        row.classList.add("ws-agent-item--pending");
      }
      const block = document.createElement("details");
      block.className = "ws-agent-item__reasoning";
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
      output.className = "ws-agent-item__output";
      output.textContent = item.text;
      row.appendChild(output);
      break;
    }
    case "error": {
      const message = document.createElement("p");
      message.className = "ws-agent-item__text";
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
 * The session surface: the transcript feed over the chat box. The
 * toolbar (mode chip, model picker, context ring) mounts into the box's
 * controls slot only when the composition root threads a ModelService
 * through; a view built without one mounts none. The box is editable
 * only while a wait is pinned; a configured model service marks the send
 * action blocked until its current selection is non-empty. A send
 * answers the wait through the service and clears the box on success.
 * The status sink receives dictation's local messages, selection
 * blockers, and recording LED state.
 */
export class AgentSessionView extends Disposable {
  readonly element: HTMLElement;
  /**
   * The chat box under the feed. Exposed so tests can drive content and
   * selection - the DOM alone sets neither on a ProseMirror editor.
   */
  readonly chatBox: ChatBox;
  private readonly feed: HTMLOListElement;
  private readonly stt: SttHandle;
  private rendered: RenderedRow[] = [];

  constructor(
    private readonly service: AgentSessionService,
    private readonly status: SttStatus,
    private readonly modelService?: ModelService,
    speechCapture?: SpeechCaptureService,
  ) {
    super();
    this.element = document.createElement("section");
    this.element.className = "ws-agent-session";
    this.element.setAttribute("aria-label", "Agent session");

    this.feed = document.createElement("ol");
    this.feed.className = "ws-agent-session__feed";
    // A live list, not role="log": the role would replace the list
    // semantics, and the property alone announces appended rows.
    this.feed.setAttribute("aria-live", "polite");
    this.feed.setAttribute("aria-atomic", "false");

    // The box is composed from what the host resolves: the toolbar for
    // its controls slot and the text-control registrar; the box itself
    // touches no registry.
    const boxProps: {
      -readonly [K in keyof ChatBoxProps]: ChatBoxProps[K];
    } = {
      placeholder: "Plan, Build, / for skills, @ for context",
      ariaLabel: "Message",
      mic: "idle",
    };
    if (modelService !== undefined) {
      boxProps.controls = this._register(new AgentToolbar(modelService)).element;
    }
    const textControls = getServiceOrNull(TEXT_CONTROL_SERVICE);
    if (textControls !== null) {
      boxProps.textControls = textControls.register.bind(textControls);
    }
    const chatBox = new ChatBox(boxProps, (event) => this.onChatBoxEvent(event));
    const outer = document.createElement("div");
    outer.className = "ws-agent-session__outer";
    outer.appendChild(chatBox.element);
    this.element.append(this.feed, outer);

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
    if (this.modelService !== undefined) {
      this._register(this.modelService.onDidChangeCurrent(() => this.renderInputState()));
    }

    // The dictation control over the box. Registered before the box so
    // disposal discards a live take while the editor still stands.
    // Production injects the composition root's capture service; isolated
    // views own a fallback for tests and previews.
    const capture = speechCapture ?? new SpeechCaptureService();
    this.stt = this._register(
      setupStt({ input: chatBox }, status, () => {
        if (this.service.pendingInputToken === null) {
          return "The agent isn't asking for input; the mic opens when it does.";
        }
        return null;
      }, capture),
    );
    if (speechCapture === undefined) {
      this._register(capture);
    }
    // Dictation's state is the box's mic prop: seeded once the handle
    // exists (the box had to come first, the handle needs it as its
    // target), then driven by every change.
    chatBox.update({ mic: this.stt.state });
    this._register(this.stt.onState((state) => chatBox.update({ mic: state })));
    this.chatBox = this._register(chatBox);

    this.renderFeed();
    this.renderInputState();
  }

  /** The box's events: a send answers the wait, a mic press drives dictation. */
  private onChatBoxEvent(event: ChatBoxEvent): void {
    switch (event.type) {
      case "send":
        this.submit();
        return;
      case "mic-press":
        this.stt.press();
        return;
      case "command":
      case "stop":
      case "cancel":
      case "mic-release":
        // Not produced by the box in this configuration; reserved.
        return;
      default: {
        const exhaustive: never = event;
        return exhaustive;
      }
    }
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

  /**
   * Pins the box to the pending wait: editable only while one is open,
   * the send action idle without one, and blocked (still clickable, so
   * the press can say why) while a configured model service has no
   * selection.
   */
  private renderInputState(): void {
    const pinned = this.service.pendingInputToken !== null;
    this.chatBox.update({
      editable: pinned,
      action: pinned
        ? this.modelService === undefined || this.modelService.current !== ""
          ? "send"
          : "send-blocked"
        : "idle",
    });
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
    const text = this.chatBox.getText();
    if (text === "" || this.service.pendingInputToken === null) {
      return;
    }
    if (this.modelService !== undefined && this.modelService.current === "") {
      this.status.showLocal("Select a model before sending.", "info");
      return;
    }
    // Read before discarding: the discard restores the box to its
    // pre-take text, and the send submits what was showing.
    this.stt.discardIfRecording();
    if (this.service.respond(text)) {
      this.chatBox.clear();
    }
  }
}
