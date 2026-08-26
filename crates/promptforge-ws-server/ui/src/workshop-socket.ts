// The persistent workshop socket: one WebSocket to /ws carries every
// downstream JSON frame - chat replies for in-flight generations and
// unsolicited status updates from the server's observer. Chat requests are
// multiplexed by an incrementing id the server echoes on that chat's
// delta/done/error frames; the UI runs one chat at a time, so the pending
// map holds at most one entry in practice.

/** One observer status update, as sent by the server. */
export interface StatusFrame {
  type: "status";
  label: string;
  description: string;
  severity: "info" | "debug" | "error";
  activity: "general" | "thinking" | "generating";
  progress: { current: number; total: number } | null;
}

/** One entry of the gateway's model catalog, as fetched or pushed. */
export interface CatalogModel {
  id: string;
  description?: string;
}

/** A pushed model catalog, sent when the gateway comes back after an outage. */
export interface ModelsFrame {
  type: "models";
  models: CatalogModel[];
}

/** The chat payload sent upstream in one `{"type":"chat",...}` frame. */
export interface ChatPayload {
  model: string;
  messages: Array<{ role: string; content: string }>;
}

interface PendingChat {
  onDelta: (content: string) => void;
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

export class WorkshopSocket {
  private socket: WebSocket | null = null;
  private opening: { socket: WebSocket; promise: Promise<void> } | null = null;
  private nextId = 1;
  private reconnectDelayMs = RECONNECT_INITIAL_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly pending = new Map<number, PendingChat>();
  private readonly statusHandlers = new Set<(frame: StatusFrame) => void>();
  private readonly modelsHandlers = new Set<(models: CatalogModel[]) => void>();
  private readonly disconnectHandlers = new Set<() => void>();
  private readonly abortHandlers = new Set<() => void>();

  constructor(private readonly url: string = defaultUrl()) {}

  /** Opens the socket unless it is already open or opening. */
  connect(): void {
    // A failed open is ignored here: `onerror` has already reset the state,
    // and the next `streamChat` retries through `ensureOpen`.
    void this.ensureOpen().catch(() => {});
  }

  /** Registers a handler for unsolicited status frames. */
  onStatus(handler: (frame: StatusFrame) => void): void {
    this.statusHandlers.add(handler);
  }

  /** Registers a handler for pushed model catalogs. */
  onModels(handler: (models: CatalogModel[]) => void): void {
    this.modelsHandlers.add(handler);
  }

  /** Registers a handler fired when the socket disconnects. */
  onDisconnect(handler: () => void): void {
    this.disconnectHandlers.add(handler);
  }

  /**
   * Registers a handler fired when an in-flight chat is aborted. The
   * recycled socket cannot see the server's terminal status frame for the
   * aborted chat, so listeners must clear local activity state themselves.
   */
  onAbort(handler: () => void): void {
    this.abortHandlers.add(handler);
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
    onDelta: (content: string) => void,
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
        for (const handler of this.abortHandlers) {
          handler();
        }
      };
      const finish = (): void => signal.removeEventListener("abort", onAbort);
      this.pending.set(id, {
        onDelta,
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
      for (const handler of this.disconnectHandlers) {
        handler();
      }
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
      const status = frame as unknown as StatusFrame;
      for (const handler of this.statusHandlers) {
        handler(status);
      }
      return;
    }
    if (frame.type === "models") {
      const models = Array.isArray(frame.models) ? (frame.models as CatalogModel[]) : [];
      for (const handler of this.modelsHandlers) {
        handler(models);
      }
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
