// The workshop side of the config panel's postMessage bridge. The
// gateway config UI runs in an iframe proxied through the workshop
// server at /gateway/config/, so the frame shares the workshop's own
// origin; this window-level listener is its only path to the workshop:
// API-forward requests go through the workshop server's key-attaching
// proxy, action notifications land on the status bar, and the ready
// announcement is answered with a context message (theme and initial
// route). Origins are pinned in both directions: a message is handled
// only when event.origin equals the workshop's own origin, and every
// reply is posted with that exact origin as its targetOrigin - never "*".

import { toDisposable, type IDisposable } from "../base/lifecycle";
import { forwardGatewayRequest, type FetchLike } from "../services/gateway-config-api";

/** The status surface the panel's action notifications land on. */
export interface BridgeStatusSink {
  /** Shows a local (client-originated) status line. */
  showLocal(label: string, severity: "info" | "error"): void;
}

/** Construction seams for {@link setupGatewayConfigBridge}. */
export interface GatewayConfigBridgeOptions {
  /** Where action notifications (apply, revert, download-started) land. */
  readonly statusBar: BridgeStatusSink;
  /** Transport for the API-forward proxy; the global fetch in production. */
  readonly fetchFn?: FetchLike;
  /** The window whose message events carry the bridge; the global one in production. */
  readonly win?: Pick<Window, "addEventListener" | "removeEventListener">;
  /**
   * Reply seam: posts `message` back to the event's source window at
   * `targetOrigin`. Tests substitute a recorder; production posts to
   * event.source with the pinned workshop origin.
   */
  readonly reply?: (event: MessageEvent, message: unknown, targetOrigin: string) => void;
}

/** Status bar lines for the actions the config UI announces. */
const ACTION_LABELS: Readonly<Record<string, string>> = {
  apply: "Gateway configuration applied",
  revert: "Gateway configuration changes reverted",
  "download-started": "Gateway download started",
};

/** The context handed to the iframe once it announces itself. */
const PANEL_CONTEXT = { type: "pf-context", theme: "dark", route: "#/local" } as const;

/**
 * Installs the window-level message listener that serves the config
 * panel's iframe. The iframe is proxied same-origin through the
 * workshop server, so messages are accepted from - and replies pinned
 * to - the workshop's own origin. The returned dispose() detaches the
 * listener.
 */
export function setupGatewayConfigBridge(options: GatewayConfigBridgeOptions): IDisposable {
  const win = options.win ?? window;
  const reply =
    options.reply ??
    ((event: MessageEvent, message: unknown, targetOrigin: string): void => {
      (event.source as Window | null)?.postMessage(message, targetOrigin);
    });
  // The panel iframe is served through the workshop's own proxy, so the
  // one legitimate sender shares this window's origin; anything else -
  // the gateway's own port included - is foreign.
  const origin = window.location.origin;

  const onMessage = (event: MessageEvent): void => {
    void (async () => {
      if (event.origin !== origin) {
        return; // Not the config panel; every foreign message is dropped.
      }
      const data = event.data as Record<string, unknown> | null;
      if (data === null || typeof data !== "object") {
        return;
      }
      switch (data["type"]) {
        case "pf-bridge-ready": {
          reply(event, PANEL_CONTEXT, origin);
          return;
        }
        case "pf-api": {
          const id = data["id"];
          const method = data["method"];
          const path = data["path"];
          const body = data["body"];
          if (typeof id !== "string" || typeof method !== "string" || typeof path !== "string") {
            return; // A malformed request has no id to answer under.
          }
          const result = await forwardGatewayRequest(
            { id, method, path, body: typeof body === "string" ? body : null },
            options.fetchFn,
          );
          reply(event, { type: "pf-api-result", id, ...result }, origin);
          return;
        }
        case "pf-action": {
          const action = data["action"];
          const label = typeof action === "string" ? ACTION_LABELS[action] : undefined;
          if (label !== undefined) {
            options.statusBar.showLocal(label, "info");
          }
          return;
        }
        default:
          return; // Unknown message kinds are dropped, never answered.
      }
    })();
  };

  win.addEventListener("message", onMessage);
  return toDisposable(() => win.removeEventListener("message", onMessage));
}
