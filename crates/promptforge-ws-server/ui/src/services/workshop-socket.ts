// The persistent workshop socket: one WebSocket to /ws carries every
// downstream JSON frame - chat replies for in-flight generations and
// unsolicited status updates from the server's observer. Chat requests are
// multiplexed by an incrementing id the server echoes on that chat's
// delta/done/error frames; the UI runs one chat at a time, so the pending
// map holds at most one entry in practice. The frame shapes themselves live
// in protocol.ts.

import { Emitter, type Event } from "../base/event";
import { Disposable, toDisposable } from "../base/lifecycle";
import type { CatalogModel, ChatPayload, StatusFrame } from "./protocol";

/** The per-chat stream callbacks handed to `streamChat`. */
export interface ChatStreamHandlers {
  /** Called for each answer-content delta. */
  onDelta: (content: string) => void;
  /** Called for each reasoning side-channel delta, when the model has one. */
  onReasoning?: (content: string) => void;
}

interface PendingChat {
  onDelta: (content: string) => void;
  onReasoning: ((content: string) => void) | undefined;
  resolve: () => void;
  reject: (error: Error) => void;
  started: boolean;
  settled: boolean;
}

interface ServerFrame {
  type?: unknown;
  id?: unknown;
  content?: unknown;
  message?: unknown;
  models?: unknown;
}

function defaultUrl(): string {
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
}

// Reconnect backoff: the first retry waits a second, each failure doubles
// it, and the cap keeps a down server from pushing the wait past 30 s.
const RECONNECT_INITIAL_MS = 1000;
const RECONNECT_MAX_MS = 30_000;

/**
 * Most pushes a boot queue will ever hold before `ready()` releases it.
 * When full, the oldest push is dropped: a newer status or catalog frame
 * supersedes an older one, and an unbounded queue on a socket whose owner
 * never declares readiness is a leak.
 */
export const BOOT_QUEUE_CAP = 32;

type QueuedPush =
  | { kind: "status"; frame: StatusFrame }
  | { kind: "models"; models: CatalogModel[] };

/**
 * Unsolicited server pushes (status, models) that arrive before `ready()`
 * is called are held in a queue bounded at `BOOT_QUEUE_CAP` (drop-oldest)
 * and replayed in arrival order when the composition root declares itself
 * ready - handlers attached at different points of boot would otherwise
 * race the server's first pushes.
 * After `ready()`, pushes deliver immediately. Chat reply frames are never
 * queued: they answer a `streamChat` call, which implies a running app.
 */
export class WorkshopSocket extends Disposable {
  private socket: WebSocket | null = null;
  private opening: { socket: WebSocket; promise: Promise<void> } | null = null;
  private nextId = 1;
  private reconnectDelayMs = RECONNECT_INITIAL_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly pending = new Map<number, PendingChat>();
  private isReady = false;
  private readonly bootQueue: QueuedPush[] = [];

  private readonly _onStatus = this._register(new Emitter<StatusFrame>());
  /** Fires for every unsolicited status frame; subscribing returns the severing disposable. */
  readonly onStatus: Event<StatusFrame> = this._onStatus.event;

  private readonly _onModels = this._register(new Emitter<CatalogModel[]>());
  /** Fires for every pushed model catalog. */
  readonly onModels: Event<CatalogModel[]> = this._onModels.event;

  private readonly _onDisconnect = this._register(new Emitter<void>());
  /** Fires when the socket disconnects. */
  readonly onDisconnect: Event<void> = this._onDisconnect.event;

  private readonly _onAbort = this._register(new Emitter<void>());
  /**
   * Fires when an in-flight chat is aborted. The recycled socket cannot
   * see the server's terminal status frame for the aborted chat, so
   * listeners must clear local activity state themselves.
   */
  readonly onAbort: Event<void> = this._onAbort.event;

  constructor(private readonly url: string = defaultUrl()) {
    super();
    // Disposal silences the socket before closing it (onclose detached the
    // way reopen() does), so teardown is never mistaken for a dropout: no
    // disconnect fan-out, no reconnect backoff. In-flight chats settle the
    // same way a close would, so no caller awaits a reply forever.
    this._register(
      toDisposable(() => {
        if (this.reconnectTimer !== null) {
          clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
        }
        const socket = this.socket;
        if (socket) {
          socket.onclose = null;
          socket.close();
          this.socket = null;
        }
        this.settleAll();
        this.bootQueue.length = 0;
      }),
    );
  }

  /** Opens the socket unless it is already open or opening. */
  connect(): void {
    // A failed open is ignored here: `onerror` has already reset the state,
    // and the next `streamChat` retries through `ensureOpen`.
    void this.ensureOpen().catch(() => {});
  }

  /**
   * Declares the app ready for pushes: replays every queued status/models
   * push in arrival order, then delivers later pushes immediately.
   * Idempotent. The composition root calls this once its handlers and
   * panels are wired.
   */
  ready(): void {
    if (this.isReady) return;
    // Readiness flips before the flush: a push a handler delivers
    // re-entrantly must emit immediately, not land in a queue that has
    // already drained and will never flush again.
    this.isReady = true;
    for (const push of this.bootQueue.splice(0)) {
      this.emitPush(push);
    }
  }

  /**
   * Sends one id-tagged chat frame and resolves when its `done` frame
   * arrives. Rejects on an `error` frame, or on a socket close before any
   * content streamed; a close after content started resolves, mirroring an
   * SSE body that ends early. Aborting the signal detaches the chat and
   * recycles the socket, which is what makes the server drop the orphaned
   * gateway stream.
   */
  async streamChat(
    payload: ChatPayload,
    handlers: ChatStreamHandlers,
    signal: AbortSignal,
  ): Promise<void> {
    await this.ensureOpen();
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error("the workshop socket is not open");
    }
    const id = this.nextId++;
    await new Promise<void>((resolve, reject) => {
      const onAbort = (): void => {
        if (!this.pending.has(id)) return;
        this.settle(id, (chat) => chat.resolve());
        this.reopen();
        this._onAbort.fire(undefined);
      };
      const finish = (): void => signal.removeEventListener("abort", onAbort);
      this.pending.set(id, {
        onDelta: handlers.onDelta,
        onReasoning: handlers.onReasoning,
        resolve: () => {
          finish();
          resolve();
        },
        reject: (error: Error) => {
          finish();
          reject(error);
        },
        started: false,
        settled: false,
      });
      signal.addEventListener("abort", onAbort, { once: true });
      try {
        socket.send(JSON.stringify({ type: "chat", id, ...payload }));
      } catch (error) {
        this.settle(id, (chat) =>
          chat.reject(error instanceof Error ? error : new Error(String(error))),
        );
      }
    });
  }

  private ensureOpen(): Promise<void> {
    if (this.socket?.readyState === WebSocket.OPEN) {
      return Promise.resolve();
    }
    if (this.opening) {
      return this.opening.promise;
    }
    const socket = new WebSocket(this.url);
    this.socket = socket;
    const entry = { socket, promise: Promise.resolve() };
    entry.promise = new Promise<void>((resolve, reject) => {
      socket.onopen = () => {
        if (this.opening === entry) this.opening = null;
        if (this.reconnectTimer !== null) {
          clearTimeout(this.reconnectTimer);
          this.reconnectTimer = null;
        }
        this.reconnectDelayMs = RECONNECT_INITIAL_MS;
        resolve();
      };
      // A failure while opening rejects the waiters; a failure on an
      // established socket is followed by close, which settles pendings.
      socket.onerror = () => {
        if (this.socket === socket) this.socket = null;
        if (this.opening === entry) this.opening = null;
        reject(new Error("the workshop socket failed to open"));
      };
    });
    this.opening = entry;
    socket.onmessage = (event: MessageEvent) => this.route(event);
    socket.onclose = () => {
      if (this.socket === socket) this.socket = null;
      if (this.opening === entry) this.opening = null;
      this.settleAll();
      // A dropped connection invalidates its queued pushes: replaying them
      // after the onDisconnect reset would render state from a dead socket.
      this.bootQueue.length = 0;
      this._onDisconnect.fire(undefined);
      this.scheduleReconnect();
    };
    return entry.promise;
  }

  /**
   * Schedules the next reconnect attempt with exponential backoff. One
   * timer at a time: a close while an attempt is already waiting does not
   * stack a second.
   */
  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null) {
      return;
    }
    const delay = this.reconnectDelayMs;
    this.reconnectDelayMs = Math.min(delay * 2, RECONNECT_MAX_MS);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      // A failed attempt ends in onclose, which schedules the next one.
      void this.ensureOpen().catch(() => {});
    }, delay);
  }

  /** Closes the current socket and opens a fresh one. */
  private reopen(): void {
    const socket = this.socket;
    if (socket) {
      // An intentional recycle, not a dropout: skip the disconnect
      // handlers and the reconnect backoff for this close.
      socket.onclose = null;
      socket.close();
    }
    // Same contract as `connect`: a failed reopen is retried by the next
    // `streamChat`.
    void this.ensureOpen().catch(() => {});
  }

  private route(event: MessageEvent): void {
    let frame: ServerFrame;
    try {
      frame = JSON.parse(String(event.data)) as ServerFrame;
    } catch {
      // A non-JSON frame carries no chat or status event; keep reading.
      return;
    }
    if (frame.type === "status") {
      this.deliverPush({ kind: "status", frame: frame as unknown as StatusFrame });
      return;
    }
    if (frame.type === "models") {
      const models = Array.isArray(frame.models) ? (frame.models as CatalogModel[]) : [];
      this.deliverPush({ kind: "models", models });
      return;
    }
    if (typeof frame.id !== "number") return;
    const chat = this.pending.get(frame.id);
    // A reply for a detached (aborted) chat is dropped.
    if (!chat) return;
    if (frame.type === "delta" && typeof frame.content === "string" && frame.content !== "") {
      chat.started = true;
      chat.onDelta(frame.content);
      return;
    }
    if (frame.type === "reasoning" && typeof frame.content === "string" && frame.content !== "") {
      // Reasoning counts as a started reply: a socket that closes after
      // only scratch work streamed still resolves rather than rejects.
      chat.started = true;
      chat.onReasoning?.(frame.content);
      return;
    }
    if (frame.type === "done") {
      this.settle(frame.id, (c) => c.resolve());
      return;
    }
    if (frame.type === "error") {
      this.settle(frame.id, (c) =>
        c.reject(
          new Error(
            typeof frame.message === "string" && frame.message !== ""
              ? frame.message
              : "the chat stream failed",
          ),
        ),
      );
    }
  }

  /** Queues a push before `ready()`, dropping the oldest at the cap. */
  private deliverPush(push: QueuedPush): void {
    if (this.isReady) {
      this.emitPush(push);
      return;
    }
    if (this.bootQueue.length >= BOOT_QUEUE_CAP) {
      this.bootQueue.shift();
    }
    this.bootQueue.push(push);
  }

  private emitPush(push: QueuedPush): void {
    if (push.kind === "status") {
      this._onStatus.fire(push.frame);
    } else {
      this._onModels.fire(push.models);
    }
  }

  /** Settles one pending chat exactly once and drops it from the map. */
  private settle(id: number, fn: (chat: PendingChat) => void): void {
    const chat = this.pending.get(id);
    if (!chat || chat.settled) return;
    chat.settled = true;
    this.pending.delete(id);
    fn(chat);
  }

  /** Settles every pending chat after the socket closed under it. */
  private settleAll(): void {
    for (const id of [...this.pending.keys()]) {
      this.settle(id, (chat) => {
        if (chat.started) {
          chat.resolve();
        } else {
          chat.reject(new Error("the workshop socket closed before the reply completed"));
        }
      });
    }
  }
}
