// The agent-session view: paints the service's transcript into the feed
// and pins the chat input to the pending input wait. The transcript is
// built the way Cursor draws it: buildTranscript turns the service's items
// into turns of keyed rows plus a tail status, and the TranscriptView
// reconciles them by key, so a streaming delta updates its row in place
// and an opened thought or group stays as the operator left it. Every
// content string is untrusted model-, tool-, or user-authored data:
// reply and thinking markdown renders through renderMarkdown, whose
// DOMPurify pass is the last step before the DOM; user text, tool
// arguments, and tool output land through textContent.
//
// Errors don't take rows. An error opens the composer's error popup
// (title, message, and Try again), which re-sends the last user message
// through the service while a wait is pinned.
//
// The composer is the ChatBox component, which this view embeds. It maps
// service state to the box's props (editable follows the pinned wait,
// the action follows generating, the wait, and the model selection, the
// mic follows dictation's state, the mode follows the toolbar's chip) and
// routes the box's events back - `send` answers the wait, `stop` cancels
// the turn, `mic-press` drives stt.ts, which splices the transcript into
// the box at the cursor. The one round action button is the mic while the
// box is empty and stays pressable whatever the state, so a click while
// blocked names the blocker on the status bar instead of the control
// silently disappearing. A take follows the wait it dictates into: when the
// pinned wait dies - spent by a send, cancelled by the server, or reset
// by a new session - the live take is discarded, because a take that
// cannot be sent is a trap.

import "./agent-session.css";

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import type { AgentSessionService } from "../../services/agent-session";
import type { ModelService } from "../../services/model-service";
import { getServiceOrNull } from "@workshop/platform/service-registry";
import {
  SpeechCaptureService,
  type SpeechCapturePresence,
} from "../../services/speech-capture";
import type { SttStatus } from "../../services/stt-status";
import { TOAST_STACK } from "../../services/toast-service";
import { TEXT_CONTROL_SERVICE } from "@workshop/platform/text-control-service";
import { AgentToolbar } from "./agent-toolbar";
import { buildTranscript } from "./transcript/transcript-model";
import { TranscriptView } from "./transcript/transcript-view";
import { ChatBox } from "../chatbox/chat-box";
import type { ChatBoxError, ChatBoxEvent, ChatBoxProps } from "../chatbox/types";
import { openInZone } from "../layout/zones";
import { setupStt, type SttHandle } from "../stt/stt";

const SESSION_LABEL = "Agent session";

/** The toast a turn's Copy raises. */
const COPIED_TOAST = "Message copied to clipboard";

/**
 * The panel hosting a session view, as dictation's capture presence names
 * and reveals it. A view built without one reveals the boot-time
 * singleton and names itself by its section label.
 */
export interface AgentSessionHost {
  /** The panel's `instance` param; undefined for the boot-time singleton. */
  readonly instance: string | undefined;
  /** The panel's current tab title, read on every call. */
  title(): string | undefined;
}

/** The error the popup shows: what the service folded, as the box takes it. */
interface PopupError {
  readonly title: string | null;
  readonly message: string;
}

/**
 * The session surface: the transcript feed over the chat box. The
 * toolbar (mode chip, model picker, context ring) mounts into the box's
 * controls slot only when the composition root threads a ModelService
 * through; a view built without one mounts none. The box is editable
 * only while a wait is pinned; a configured model service marks the send
 * action blocked until its current selection is non-empty. A send
 * answers the wait through the service, clears the box on success, and
 * pins the feed to the bottom.
 * The status sink receives dictation's local messages, selection
 * blockers, and recording LED state. While this view dictates, the
 * capture service names it by its session title and agent, and reveals
 * it by reopening its host panel.
 */
export class AgentSessionView extends Disposable {
  readonly element: HTMLElement;
  /**
   * The chat box under the feed. Exposed so tests can drive content and
   * selection - the DOM alone sets neither on a ProseMirror editor.
   */
  readonly chatBox: ChatBox;
  /** The transcript feed; its `element` is the scrolling region. */
  readonly transcript: TranscriptView;
  private readonly stt: SttHandle;
  private readonly toolbar: AgentToolbar | null = null;
  private popup: PopupError | null = null;
  private sessionId: string | null = null;

  constructor(
    private readonly service: AgentSessionService,
    private readonly status: SttStatus,
    private readonly modelService?: ModelService,
    speechCapture?: SpeechCaptureService,
    host?: AgentSessionHost,
  ) {
    super();
    this.element = document.createElement("section");
    this.element.className = "ws-agent-session";
    this.element.setAttribute("aria-label", SESSION_LABEL);

    this.transcript = this._register(
      new TranscriptView({
        copied: () => {
          getServiceOrNull(TOAST_STACK)?.show(COPIED_TOAST, "info");
        },
      }),
    );

    // The box is composed from what this view resolves: the toolbar for
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
      this.toolbar = this._register(new AgentToolbar(modelService));
      boxProps.controls = this.toolbar.element;
      boxProps.mode = this.toolbar.mode;
    }
    const textControls = getServiceOrNull(TEXT_CONTROL_SERVICE);
    if (textControls !== null) {
      boxProps.textControls = textControls.register.bind(textControls);
    }
    const chatBox = new ChatBox(boxProps, (event) => this.onChatBoxEvent(event));
    const outer = document.createElement("div");
    outer.className = "ws-agent-session__outer";
    outer.appendChild(chatBox.element);
    this.element.append(this.transcript.element, outer);

    // Element-owned listeners die with the elements; only service
    // subscriptions need the lifecycle.
    this._register(this.service.onDidChangeTranscript(() => this.renderFeed()));
    this._register(
      this.service.onDidChangeGenerating(() => {
        this.renderFeed();
        // Stop appears with the first generating delta and leaves with the
        // turn's last.
        this.renderInputState();
      }),
    );
    this._register(this.service.onDidChangeReconnecting(() => this.renderFeed()));
    if (this.toolbar !== null) {
      this._register(
        this.toolbar.onDidChangeMode((mode) => {
          this.chatBox.update({ mode });
        }),
      );
    }
    // Shift+Tab inside the composer opens the mode menu, and pressing it
    // again cycles. It cannot be a registered keybinding: a claimed chord is
    // swallowed everywhere, and Shift+Tab is every other control's way back.
    this.element.addEventListener("keydown", this.onKeydown);
    this._register(toDisposable(() => this.element.removeEventListener("keydown", this.onKeydown)));
    this._register(this.service.onError(() => this.showError()));
    this._register(
      this.service.onDidChangeSession((frame) => {
        // A new session replays from index zero: the old session's error
        // does not belong to it.
        if (this.sessionId !== frame.session) {
          this.sessionId = frame.session;
          this.clearError();
        }
      }),
    );
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
    const presence: SpeechCapturePresence = {
      label: () => {
        const title = host?.title() ?? SESSION_LABEL;
        const agent = this.service.session?.agent;
        return agent === undefined ? title : `${title} (${agent})`;
      },
      reveal: () => {
        const instance = host?.instance;
        openInZone("agent", instance === undefined ? {} : { instance });
      },
    };
    this.stt = this._register(
      setupStt({ input: chatBox, liveRegionHost: this.element }, status, () => {
        if (this.service.pendingInputToken === null) {
          return "The agent isn't asking for input; the mic opens when it does.";
        }
        return null;
      }, capture, presence),
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
      case "retry":
        this.retry();
        return;
      case "stop":
        this.cancelTurn();
        return;
      case "command":
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
   * Repaints the transcript from the service: the model is rebuilt from
   * the items and the transcript view reconciles it by key, so only the
   * rows that changed repaint.
   */
  private renderFeed(): void {
    const generating = this.service.generating;
    this.transcript.render(
      buildTranscript(this.service.items, generating, this.service.reconnecting),
      generating,
    );
  }

  /**
   * Pins the box to the pending wait: editable only while one is open,
   * the send action idle without one, and blocked (still clickable, so
   * the press can say why) while a configured model service has no
   * selection. While the agent generates the action is Stop, whatever the
   * wait says. The popup's Try again follows the same wait.
   */
  private renderInputState(): void {
    const pinned = this.service.pendingInputToken !== null;
    const generating = this.service.generating;
    this.chatBox.update({
      editable: pinned,
      action: generating
        ? "stop"
        : pinned
          ? this.modelService === undefined || this.modelService.current !== ""
            ? "send"
            : "send-blocked"
          : "idle",
    });
    this.renderError();
  }

  // --- The composer's commands -----------------------------------------------------------

  /**
   * Stop (the round button, Ctrl+Shift+Backspace): cancels the running
   * turn. Answers false when no turn is running or the socket is down, so
   * a chord with nothing to stop is a no-op.
   */
  cancelTurn(): boolean {
    if (!this.service.generating) {
      return false;
    }
    return this.service.cancelTurn();
  }

  /** Focuses the composer's editor. */
  focusInput(): void {
    this.chatBox.focus();
  }

  /**
   * Voice Input (Ctrl+Shift+Space): the mic's press from the keyboard. The
   * round button is the mic only over an empty box, so a take that starts
   * or ends with text already in the box goes through here.
   */
  toggleVoiceInput(): void {
    this.stt.press();
  }

  /** Whether focus sits inside this view, composer or transcript. */
  hasFocus(): boolean {
    return this.element.contains(this.element.ownerDocument.activeElement);
  }

  /** Whether the session has no turn yet and the composer holds no draft: a chat to reuse. */
  isEmpty(): boolean {
    return this.service.items.length === 0 && this.chatBox.getText() === "";
  }

  /** Opens the mode menu, or cycles to the next mode while it is open. */
  openModeMenu(): void {
    this.toolbar?.openModeMenu();
  }

  /** Opens the model menu. */
  openModelMenu(): void {
    this.toolbar?.openModelMenu();
  }

  /** Shift+Tab with focus in the composer: the mode menu. */
  private readonly onKeydown = (event: KeyboardEvent): void => {
    if (event.key !== "Tab" || !event.shiftKey || event.ctrlKey || event.altKey || event.metaKey) {
      return;
    }
    const target = event.target;
    if (!(target instanceof Element) || target.closest(".ws-prompt-input") === null) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    this.openModeMenu();
  };

  // --- The error popup ---------------------------------------------------------------

  /** Opens the popup with the error the service just folded. */
  private showError(): void {
    const last = this.service.items[this.service.items.length - 1];
    if (last?.kind !== "error") {
      return;
    }
    this.popup = { title: last.title, message: last.message };
    this.renderError();
  }

  private clearError(): void {
    this.popup = null;
    this.renderError();
  }

  /** The text of the last user message, or null before the operator has sent one. */
  private lastUserText(): string | null {
    const items = this.service.items;
    for (let index = items.length - 1; index >= 0; index--) {
      const item = items[index];
      if (item?.kind === "user") {
        return item.text;
      }
    }
    return null;
  }

  /** Pushes the popup to the box: Try again is offered after a send and enabled only while a wait is pinned. */
  private renderError(): void {
    if (this.popup === null) {
      this.chatBox.update({ error: null });
      return;
    }
    const error: { -readonly [K in keyof ChatBoxError]: ChatBoxError[K] } = {
      message: this.popup.message,
    };
    if (this.popup.title !== null) {
      error.title = this.popup.title;
    }
    if (this.lastUserText() !== null) {
      error.tryAgain = this.service.pendingInputToken !== null ? "enabled" : "disabled";
    }
    this.chatBox.update({ error });
  }

  /**
   * Try again: re-sends the last user message through the service. The
   * resend adds a second user message to the agent's history; the chat
   * agent returns to its ask after a failed round, which is the wait this
   * answers.
   */
  private retry(): void {
    const text = this.lastUserText();
    if (text === null || this.service.pendingInputToken === null) {
      return;
    }
    this.stt.discardIfRecording();
    if (this.service.respond(text)) {
      this.transcript.forcePin();
      this.clearError();
    }
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
      this.transcript.forcePin();
      this.clearError();
    }
  }
}
