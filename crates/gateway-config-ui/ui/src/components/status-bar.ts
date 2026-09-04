// The fixed bottom status bar [VS Code], built on the shared shell
// (shared-ui/status-bar): the shell owns the bar, the text region, and
// the slot's progress/indicators swap; this component populates them
// from the extended GET /admin/status response. Idle shows the endpoint
// LED strip (green ready, amber provisioning, gray unconfigured) in the
// indicators group plus the model count and declared VRAM in the extras
// region; an active queue command swaps the slot to the progress bar,
// puts the command label in the text, and fills the extras region with
// the pending count, one cancel button per pending command (POST
// /admin/queue/cancel-pending), and the active command's cancel button
// (POST /admin/queue/cancel). Self-contained on purpose - it owns its
// poll loop and the body class that keeps page content clear of the
// fixed strip.

import { X, createElement as lucideElement } from "lucide";
import { createStatusBarShell } from "shared-ui/status-bar";

import type { EndpointStatus, GatewayApi, GatewayStatus } from "../services/gateway-api";

/** The status poll cadence; the bar is the shell's only live status consumer. */
const STATUS_POLL_MS = 2000;

/** Construction dependencies for the status bar. */
export interface StatusBarOptions {
  /** The gateway client; the bar polls `getStatus` and cancels through it. */
  api: GatewayApi;
  /** Poll cadence override, for tests. */
  pollMs?: number;
  /** Reports a cancel failure; the composition root routes it to the toasts. */
  onError?: (message: string) => void;
}

/** The mounted status bar and its lifecycle handles. */
export interface StatusBar {
  /** The `<footer class="status-bar">` element; the chrome appends it once. */
  element: HTMLElement;
  /** Starts the poll loop and reserves the page space the bar overlays. */
  start(): void;
  /** Stops the poll loop and releases the reserved page space. */
  stop(): void;
  /** Applies one status response; exposed so tests can drive the swap directly. */
  update(status: GatewayStatus): void;
}

/** The LED state an endpoint entry maps to. */
function ledState(endpoint: EndpointStatus): "ready" | "provisioning" | "unconfigured" {
  if (endpoint.ready) {
    return "ready";
  }
  return endpoint.provisioning ? "provisioning" : "unconfigured";
}

/** The idle-state summary: the model count plus declared VRAM when nonzero. */
function summaryText(models: number, vramGb: number): string {
  const noun = models === 1 ? "1 model" : `${models} models`;
  return vramGb > 0 ? `${noun}, ${vramGb.toFixed(1)} GB` : noun;
}

/** Creates the status bar. */
export function createStatusBar(options: StatusBarOptions): StatusBar {
  const shell = createStatusBarShell();
  const element = shell.element;

  // Idle state: the endpoint LED strip fills the shell's indicators
  // group; the model/VRAM summary sits in the extras region.
  const leds = document.createElement("div");
  leds.className = "status-leds";
  shell.indicators.append(leds);
  const summary = document.createElement("span");
  summary.className = "status-bar-summary";

  // Active state: the extras region's queue group holds the pending
  // count, one cancel button per waiting command, and the active
  // command's cancel button.
  const queueGroup = document.createElement("span");
  queueGroup.className = "status-bar-queue";
  queueGroup.hidden = true;
  const pendingNote = document.createElement("span");
  pendingNote.className = "status-bar-pending";
  pendingNote.hidden = true;
  const pendingList = document.createElement("span");
  pendingList.className = "status-bar-pending-list";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "button button-xs button-outline status-bar-cancel";
  cancel.textContent = "Cancel";
  queueGroup.append(pendingNote, pendingList, cancel);
  shell.extras.append(summary, queueGroup);

  let timer: ReturnType<typeof setInterval> | null = null;

  const poll = async (): Promise<void> => {
    try {
      bar.update(await options.api.getStatus());
    } catch {
      // An unreachable gateway already reddened the connection dot, and a
      // 401 already routed to the key prompt; the next poll retries.
    }
  };

  cancel.addEventListener("click", () => {
    cancel.disabled = true;
    void (async () => {
      try {
        await options.api.cancelActiveCommand();
        // Refresh at once so the bar does not wait a poll cadence to
        // show the cancellation.
        await poll();
      } catch (error) {
        options.onError?.(error instanceof Error ? error.message : "The cancel failed");
      } finally {
        cancel.disabled = false;
      }
    })();
  });

  const bar: StatusBar = {
    element,
    start(): void {
      document.body.classList.add("has-status-bar");
      void poll();
      timer = setInterval(() => void poll(), options.pollMs ?? STATUS_POLL_MS);
      // Node's interval carries `unref`; the browser's numeric handle does
      // not, so a test process never hangs on the bar's poll loop.
      (timer as unknown as { unref?: () => void }).unref?.();
    },
    stop(): void {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
      document.body.classList.remove("has-status-bar");
    },
    update(status: GatewayStatus): void {
      const active = status.queue.active;
      if (active !== null) {
        const fraction = Math.min(Math.max(active.fraction, 0), 1);
        const percent = Math.round(fraction * 100);
        shell.setText(`${active.name} (${percent}%)`);
        shell.renderSlot({ current: percent, total: 100 });
        summary.hidden = true;
        queueGroup.hidden = false;
        const pendingCount = status.queue.pending.length;
        pendingNote.hidden = pendingCount === 0;
        pendingNote.textContent =
          pendingCount === 1 ? "1 queued" : `${pendingCount} queued`;
        pendingList.replaceChildren(
          ...status.queue.pending.map((entry, index) => {
            const button = document.createElement("button");
            button.type = "button";
            button.className = "button button-xs button-outline status-bar-pending-cancel";
            button.setAttribute("aria-label", `Cancel the queued ${entry.name}`);
            button.title = `Cancel the queued ${entry.name}`;
            const icon = lucideElement(X, { "aria-hidden": "true", width: 12, height: 12 });
            const name = document.createElement("span");
            name.textContent = entry.name;
            button.append(icon, name);
            button.addEventListener("click", () => {
              button.disabled = true;
              void (async () => {
                try {
                  await options.api.cancelPendingCommand(index);
                  // Refresh at once so the bar does not wait a poll
                  // cadence to show the cancellation.
                  await poll();
                } catch (error) {
                  options.onError?.(
                    error instanceof Error ? error.message : "The cancel failed",
                  );
                } finally {
                  button.disabled = false;
                }
              })();
            });
            return button;
          }),
        );
        return;
      }
      shell.setText("");
      shell.renderSlot(null);
      summary.hidden = false;
      queueGroup.hidden = true;
      leds.replaceChildren(
        ...status.endpoints.map((endpoint) => {
          const state = ledState(endpoint);
          const led = document.createElement("span");
          led.className = "status-led";
          led.dataset.state = state;
          led.title = `${endpoint.name} (${endpoint.path}): ${state}`;
          const dot = document.createElement("span");
          dot.className = "status-led-dot";
          dot.setAttribute("aria-hidden", "true");
          const name = document.createElement("span");
          name.className = "status-led-name";
          name.textContent = endpoint.name;
          const narration = document.createElement("span");
          narration.className = "visually-hidden";
          narration.textContent = state;
          led.append(dot, name, narration);
          return led;
        }),
      );
      summary.textContent = summaryText(status.models.length, status.vram_gb);
    },
  };
  return bar;
}
