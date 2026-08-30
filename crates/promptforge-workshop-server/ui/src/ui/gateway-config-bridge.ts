// The workshop side of the config panel's postMessage bridge. The
// gateway config UI runs in an iframe on the gateway's origin (a
// different port), so this window-level listener is its only path to
// the workshop: API-forward requests go through the workshop server's
// key-attaching proxy, action notifications land on the status bar, and
// the ready announcement is answered with a context message (theme and
// initial route). Origins are pinned in both directions: a message is
// handled only when event.origin equals the gateway origin the server
// reported, and every reply is posted with that exact origin as its
// targetOrigin - never "*".

import { toDisposable, type IDisposable } from "../base/lifecycle";
import {
  fetchGatewayOrigin,
  forwardGatewayRequest,
  type FetchLike,
} from "../services/gateway-config-api";

/** The status surface the panel's action notifications land on. */
export interface BridgeStatusSink {
  /** Shows a local (client-originated) status line. */
  showLocal(label: string, severity: "info" | "error"): void;
}

/** Construction seams for {@link setupGatewayConfigBridge}. */
export interface GatewayConfigBridgeOptions {
  /** Where action notifications (apply, revert, download-started) land. */
  readonly statusBar: BridgeStatusSink;
  /** Transport for the origin probe and the proxy; the global fetch in production. */
  readonly fetchFn?: FetchLike;
  /** The window whose message events carry the bridge; the global one in production. */
  readonly win?: Pick<Window, "addEventListener" | "removeEventListener">;
  /**
   * Reply seam: posts `message` back to the event's source window at
   * `targetOrigin`. Tests substitute a recorder; production posts to
   * event.source with the pinned gateway origin.
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
const PANEL_CONTEXT = { type: "pf-context", theme: "dark", route: "#/models" } as const;

/**
 * Installs the window-level message listener that serves the config
 * panel's iframe. The gateway origin resolves lazily on the first
 * message - a workshop that never opens the panel never dials the
 * server - and a failed probe retries on the next message. The returned
 * dispose() detaches the listener.
 */
export function setupGatewayConfigBridge(options: GatewayConfigBridgeOptions): IDisposable {
  const win = options.win ?? window;
  const reply =
    options.reply ??
    ((event: MessageEvent, message: unknown, targetOrigin: string): void => {
      (event.source as Window | null)?.postMessage(message, targetOrigin);
    });
  let originProbe: Promise<string | null> | null = null;

  const onMessage = (event: MessageEvent): void => {
    void (async () => {
      originProbe ??= fetchGatewayOrigin(options.fetchFn);
      const origin = await originProbe;
      if (origin === null) {
        // The probe failed; retry on the next message instead of
        // wedging the bridge for the page's lifetime.
        originProbe = null;
        return;
      }
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
