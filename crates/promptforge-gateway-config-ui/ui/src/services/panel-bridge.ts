// The config UI's side of the workshop panel bridge. In panel mode the
// SPA runs in an iframe inside the workshop and holds no bearer key:
// every gateway call becomes a postMessage to the parent, which the
// workshop forwards through its server-side proxy with the key
// attached, answering with a correlated result message. Origins are
// pinned in both directions: the workshop's origin arrives in the
// iframe URL's `bridge` parameter, only messages from that exact origin
// are accepted, and every outgoing message targets it - never "*".

import type { FetchLike } from "./gateway-api";

/** The context the workshop sends once the bridge is up. */
export interface PanelContext {
  /** The workshop's theme name. */
  theme: string;
  /** The initial route hash ("#/models"), or "" when the workshop sent none. */
  route: string;
}

/** The actions the shell announces to the workshop's status bar. */
export type PanelAction = "apply" | "revert" | "download-started";

/** The window surface the bridge needs; tests hand in a jsdom window. */
export interface BridgeWindow {
  /** Message-event registration. */
  addEventListener(type: string, listener: (event: MessageEvent) => void): void;
  /** Message-event removal, so a disposed bridge leaves no listener. */
  removeEventListener(type: string, listener: (event: MessageEvent) => void): void;
  /** The embedding workshop window. */
  parent: { postMessage(message: unknown, targetOrigin: string): void };
}

/** Construction options for {@link PanelBridge}. */
export interface PanelBridgeOptions {
  /** The window carrying the message events. */
  win: BridgeWindow;
  /** The pinned workshop origin, from the iframe URL's `bridge` parameter. */
  origin: string;
  /** Outgoing-post seam; production posts to the parent at the pinned origin. */
  post?: (message: unknown) => void;
  /** Reply deadline per bridged call; the default covers everything but downloads. */
  timeoutMs?: number;
}

/** Reply deadline for ordinary bridged calls. */
const DEFAULT_TIMEOUT_MS = 30_000;

/**
 * Reply deadline for `POST /v1/cache`: the proxy buffers the download's
 * whole SSE stream and answers only at the end, so the deadline covers
 * a multi-gigabyte model download. (An explicit `timeoutMs` option
 * overrides both tiers, so tests can trip them quickly.)
 */
const CACHE_TIMEOUT_MS = 30 * 60_000;

interface PendingCall {
  resolve: (response: Response) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/**
 * Parses the iframe URL's `bridge` parameter into a pinned http(s)
 * origin, or null when it is absent, malformed, or not a loopback host -
 * the shell then stays in its inert bridge-pending state. The bridge
 * origin selects the postMessage targetOrigin for every gateway call, so
 * it must be the loopback workshop and never a foreign origin: a crafted
 * `?bridge=https://evil.example` in a framed copy of this loopback-served
 * SPA would otherwise aim the frame's API traffic (write bodies, typed
 * secrets included) at that origin. The workshop listener binds loopback
 * only, so a non-loopback bridge is never legitimate.
 */
export function parseBridgeOrigin(raw: string | null): string | null {
  if (raw === null || raw === "") {
    return null;
  }
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return null;
    }
    return isLoopbackHost(url.hostname) ? url.origin : null;
  } catch {
    return null;
  }
}

/** Whether `hostname` (as `URL.hostname` reports it) is a loopback host. */
function isLoopbackHost(hostname: string): boolean {
  if (hostname === "localhost" || hostname === "[::1]" || hostname === "::1") {
    return true;
  }
  // IPv4 loopback is the whole 127.0.0.0/8 block.
  const octets = hostname.split(".");
  return (
    octets.length === 4 &&
    octets[0] === "127" &&
    octets.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
  );
}

/** The postMessage transport standing in for fetch in panel mode. */
export class PanelBridge {
  /** Fired with the workshop's context message (theme, initial route). */
  onContext: ((context: PanelContext) => void) | null = null;

  /** The transport handed to GatewayApi in place of fetch. */
  readonly fetchLike: FetchLike;

  private readonly win: BridgeWindow;
  private readonly origin: string;
  private readonly post: (message: unknown) => void;
  private readonly timeoutMs: number;
  private readonly cacheTimeoutMs: number;
  private readonly pending = new Map<string, PendingCall>();
  private nextId = 0;

  constructor(options: PanelBridgeOptions) {
    this.win = options.win;
    this.origin = options.origin;
    this.post =
      options.post ?? ((message) => this.win.parent.postMessage(message, this.origin));
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.cacheTimeoutMs = options.timeoutMs ?? CACHE_TIMEOUT_MS;
    this.fetchLike = (input, init) => this.request(input, init);
  }

  /** Attaches the message listener and announces the iframe is ready. */
  start(): void {
    this.win.addEventListener("message", this.onMessage);
    this.post({ type: "pf-bridge-ready" });
  }

  /** Announces one shell action for the workshop's status bar. */
  notifyAction(action: PanelAction): void {
    this.post({ type: "pf-action", action });
  }

  /** Detaches the listener and fails every in-flight call. */
  dispose(): void {
    this.win.removeEventListener("message", this.onMessage);
    for (const call of this.pending.values()) {
      clearTimeout(call.timer);
      call.reject(new Error("the workshop bridge closed"));
    }
    this.pending.clear();
  }

  /** Sends one bridged API call and awaits its correlated result. */
  private request(path: string, init?: RequestInit): Promise<Response> {
    const method = (init?.method ?? "GET").toUpperCase();
    const body = typeof init?.body === "string" ? init.body : null;
    this.nextId += 1;
    const id = `pf-${this.nextId}`;
    return new Promise<Response>((resolve, reject) => {
      const limit =
        method === "POST" && path.startsWith("/v1/cache") ? this.cacheTimeoutMs : this.timeoutMs;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("the workshop bridge timed out"));
      }, limit);
      this.pending.set(id, { resolve, reject, timer });
      this.post({ type: "pf-api", id, method, path, body });
    });
  }

  /** Handles one message from the pinned workshop origin. */
  private readonly onMessage = (event: MessageEvent): void => {
    if (event.origin !== this.origin) {
      return; // Every foreign message is dropped.
    }
    const data = event.data as Record<string, unknown> | null;
    if (data === null || typeof data !== "object") {
      return;
    }
    if (data["type"] === "pf-context") {
      this.onContext?.({
        theme: typeof data["theme"] === "string" ? data["theme"] : "dark",
        route: typeof data["route"] === "string" ? data["route"] : "",
      });
      return;
    }
    if (data["type"] === "pf-api-result") {
      const id = data["id"];
      if (typeof id !== "string") {
        return;
      }
      const call = this.pending.get(id);
      if (call === undefined) {
        return; // Timed out already, or never ours.
      }
      this.pending.delete(id);
      clearTimeout(call.timer);
      const status = typeof data["status"] === "number" ? data["status"] : 0;
      if (status === 0) {
        // The workshop could not reach the gateway; mirror what fetch
        // itself throws so GatewayApi's health handling sees one shape.
        call.reject(new TypeError("the workshop bridge could not reach the gateway"));
        return;
      }
      const body = typeof data["body"] === "string" ? data["body"] : "";
      const contentType = typeof data["contentType"] === "string" ? data["contentType"] : null;
      try {
        call.resolve(
          new Response(body === "" ? null : body, {
            status,
            headers: contentType === null ? {} : { "content-type": contentType },
          }),
        );
      } catch {
        // A status Response cannot represent (a bodied 204, an out-of-range
        // code) fails the call instead of throwing out of the listener.
        call.reject(new TypeError("the workshop bridge relayed an unrepresentable response"));
      }
    }
  };
}
