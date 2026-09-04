// The fixed bottom status bar [VS Code]: two mutually exclusive states
// sharing one strip. Idle shows the endpoint LED strip (green ready,
// amber provisioning, gray unconfigured) plus the model count and
// declared VRAM; an active queue command swaps the strip for a
// full-width progress bar with the command label, a cancel button
// firing POST /admin/queue/cancel, and one cancel button per pending
// command firing POST /admin/queue/cancel-pending. Driven by the
// extended GET /admin/status response. Self-contained on purpose - it
// owns its poll loop and the body class that keeps page content clear
// of the fixed strip - so the planned move to shared-ui is a file move.

import { X, createElement as lucideElement } from "lucide";

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
  const element = document.createElement("footer");
  element.className = "status-bar";

  // Idle state: the endpoint LED strip plus the model/VRAM summary.
  const idle = document.createElement("div");
  idle.className = "status-bar-idle";
  const leds = document.createElement("div");
  leds.className = "status-leds";
  const summary = document.createElement("span");
  summary.className = "status-bar-summary";
  idle.append(leds, summary);

  // Active state: the command label, a full-width progress bar, the
  // pending count with one cancel button per waiting command, and the
  // active command's cancel button.
  const activePane = document.createElement("div");
  activePane.className = "status-bar-active";
  activePane.hidden = true;
  const commandLabel = document.createElement("span");
  commandLabel.className = "status-bar-command";
  const progress = document.createElement("div");
  progress.className = "status-bar-progress";
  progress.setAttribute("role", "progressbar");
  progress.setAttribute("aria-valuemin", "0");
  progress.setAttribute("aria-valuemax", "100");
  const fill = document.createElement("div");
  fill.className = "status-bar-progress-fill";
  progress.append(fill);
  const pendingNote = document.createElement("span");
  pendingNote.className = "status-bar-pending";
  pendingNote.hidden = true;
  const pendingList = document.createElement("div");
  pendingList.className = "status-bar-pending-list";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "button button-xs button-outline status-bar-cancel";
  cancel.textContent = "Cancel";
  activePane.append(commandLabel, progress, pendingNote, pendingList, cancel);
  element.append(idle, activePane);

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
        idle.hidden = true;
        activePane.hidden = false;
        const fraction = Math.min(Math.max(active.fraction, 0), 1);
        const percent = Math.round(fraction * 100);
        commandLabel.textContent = `${active.name} (${percent}%)`;
        fill.style.setProperty("--progress", String(fraction));
        progress.setAttribute("aria-valuenow", String(percent));
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
      activePane.hidden = true;
      idle.hidden = false;
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
