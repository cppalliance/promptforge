// The persistent workshop socket: one WebSocket to /ws carries the
// server's downstream JSON - unsolicited status, catalog, and workbench
// pushes - and the inbound Model-menu events (select_model,
// switch_profile). Chat itself rides the /agents/ws socket
// (agent-socket.ts); this connection carries no chat frames. The frame
// shapes themselves live in protocol.ts.

import { Emitter, type Event } from "../base/event";
import { Disposable, toDisposable } from "../base/lifecycle";
import type { CatalogModel, StatusFrame, WorkbenchFrame } from "./protocol";

interface ServerFrame {
  type?: unknown;
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
  | { kind: "models"; models: CatalogModel[] }
  | { kind: "workbench"; frame: WorkbenchFrame };

/**
 * Unsolicited server pushes (status, models, workbench) that arrive before `ready()`
 * is called are held in a queue bounded at `BOOT_QUEUE_CAP` (drop-oldest)
 * and replayed in arrival order when the composition root declares itself
 * ready - handlers attached at different points of boot would otherwise
 * race the server's first pushes.
 * After `ready()`, pushes deliver immediately.
 */
export class WorkshopSocket extends Disposable {
  private socket: WebSocket | null = null;
  private opening: { socket: WebSocket; promise: Promise<void> } | null = null;
  private reconnectDelayMs = RECONNECT_INITIAL_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private isReady = false;
  private readonly bootQueue: QueuedPush[] = [];

  private readonly _onStatus = this._register(new Emitter<StatusFrame>());
  /** Fires for every unsolicited status frame; subscribing returns the severing disposable. */
  readonly onStatus: Event<StatusFrame> = this._onStatus.event;

  private readonly _onModels = this._register(new Emitter<CatalogModel[]>());
  /** Fires for every pushed model catalog. */
  readonly onModels: Event<CatalogModel[]> = this._onModels.event;

  private readonly _onWorkbench = this._register(new Emitter<WorkbenchFrame>());
  /** Fires for every pushed workbench snapshot. */
  readonly onWorkbench: Event<WorkbenchFrame> = this._onWorkbench.event;

  private readonly _onDisconnect = this._register(new Emitter<void>());
  /** Fires when the socket disconnects. */
  readonly onDisconnect: Event<void> = this._onDisconnect.event;

  constructor(private readonly url: string = defaultUrl()) {
    super();
    // Disposal silences the socket before closing it (onclose detached
    // first), so teardown is never mistaken for a dropout: no disconnect
    // fan-out, no reconnect backoff.
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
        this.bootQueue.length = 0;
      }),
    );
  }

  /** Opens the socket unless it is already open or opening. */
  connect(): void {
    // A failed open is ignored here: `onerror` has already reset the state,
    // and the reconnect backoff retries through `ensureOpen`.
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
      // established socket is followed by close, which reconnects.
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

  /**
   * Sends one `select_model` event frame naming the chat model. Returns
   * false without sending when the socket is down, so the caller can show
   * a status-bar error instead of the request vanishing silently; a
   * refusal from the server arrives as an error frame, not here.
   */
  selectModel(id: string): boolean {
    return this.sendFrame({ type: "select_model", model: id });
  }

  /**
   * Sends one `switch_profile` event frame starting a gateway profile
   * switch. The failure contract matches `selectModel`: false when the
   * socket is down, nothing sent.
   */
  switchProfile(name: string): boolean {
    return this.sendFrame({ type: "switch_profile", name });
  }

  /** Sends one JSON frame; false when the socket is down or the send threw. */
  private sendFrame(frame: Record<string, unknown>): boolean {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return false;
    }
    try {
      socket.send(JSON.stringify(frame));
      return true;
    } catch {
      // A send that throws mid-close is the same failure as a closed
      // socket; the close handler carries the cleanup.
      return false;
    }
  }

  private route(event: MessageEvent): void {
    let frame: ServerFrame;
    try {
      frame = JSON.parse(String(event.data)) as ServerFrame;
    } catch {
      // A non-JSON frame carries no push; keep reading.
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
    if (frame.type === "workbench") {
      this.deliverPush({ kind: "workbench", frame: frame as unknown as WorkbenchFrame });
    }
    // Error frames answer menu events; the server's status frames carry
    // the user-visible outcome, so they need no local routing.
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
    } else if (push.kind === "models") {
      this._onModels.fire(push.models);
    } else {
      this._onWorkbench.fire(push.frame);
    }
  }
}
