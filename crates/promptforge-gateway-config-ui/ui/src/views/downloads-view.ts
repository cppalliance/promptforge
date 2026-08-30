// The Downloads view: Active cards [Adapted: LM Studio] download card,
// one per in-flight download-store entry - filename, lava-gradient
// progress bar, percent, humanized speed and ETA; a failed entry shows
// its error with a Retry button that restarts through the store. The
// plan's cancel X is omitted: step 17 established the gateway has no
// cancel endpoint (a dropped stream leaves the server-side download
// running), so a cancel control would be a lie. Completed rows
// [Adapted: LM Studio] come from GET /v1/cache - green check, filename,
// size, and a Delete button (confirm, DELETE /v1/cache/{sha256},
// refresh); the cache sidecars record no timestamp, so the plan's
// relative date has no source and is omitted.

import { Check, createElement as lucideElement } from "lucide";

import { confirmDialog } from "../components/confirm-modal";
import type { ToastStack } from "../components/toast";
import { fileName, formatBytes } from "../format";
import type { CacheListEntry, GatewayApi } from "../services/gateway-api";
import { UnauthorizedError } from "../services/gateway-api";
import type { DownloadEntry, DownloadStore } from "../services/download-store";

/** Construction dependencies for the view. */
export interface DownloadsViewDeps {
  /** The admin API, for the cache listing and deletes. */
  api: GatewayApi;
  /** The global download store the Active section renders. */
  downloads: DownloadStore;
  /** Outcome surfacing. */
  toasts: ToastStack;
}

/** The mounted view handle the router calls. */
export interface DownloadsView {
  /** Renders the view into `main`. */
  mount(main: HTMLElement): void;
}

/** Builds the Downloads view (state survives route re-mounts). */
export function createDownloadsView(deps: DownloadsViewDeps): DownloadsView {
  const { api, downloads, toasts } = deps;

  let main: HTMLElement | null = null;
  let activeBox: HTMLElement | null = null;
  let completedBox: HTMLElement | null = null;
  /** The cache listing; null until the first fetch answers. */
  let completed: CacheListEntry[] | null = null;
  let listError: string | null = null;

  const refreshList = async (): Promise<void> => {
    try {
      completed = await api.listCache();
      listError = null;
    } catch (error) {
      if (error instanceof UnauthorizedError) {
        // The key prompt already took over the screen.
        return;
      }
      listError = error instanceof Error ? error.message : String(error);
    }
    renderCompleted();
  };

  // A completed download becomes a cache entry, so the Completed
  // section refreshes when the set of ready entries grows; byte-level
  // progress only redraws the Active section.
  let readySignature = "";
  downloads.subscribe(() => {
    if (activeBox?.isConnected) {
      renderActive();
    }
    const signature = downloads
      .entries()
      .filter((entry) => entry.status === "ready")
      .map((entry) => entry.source)
      .join("|");
    if (signature !== readySignature) {
      readySignature = signature;
      if (completedBox?.isConnected) {
        void refreshList();
      }
    }
  });

  // ----- the Active section -------------------------------------------------

  const renderActive = (): void => {
    if (!activeBox) {
      return;
    }
    const entries = downloads.entries().filter((entry) => entry.status !== "ready");
    if (entries.length === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "No active downloads.";
      activeBox.replaceChildren(empty);
      return;
    }
    activeBox.replaceChildren(...entries.map(downloadCard));
  };

  const downloadCard = (entry: DownloadEntry): HTMLElement => {
    const card = document.createElement("article");
    card.className = "download-card";
    const name = document.createElement("p");
    name.className = "download-name";
    name.textContent = entry.label;
    card.append(name);

    if (entry.status === "error") {
      const message = document.createElement("p");
      message.className = "download-error";
      message.textContent = `The download failed: ${entry.message ?? "unknown error"}`;
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "button button-xs button-outline download-retry";
      retry.textContent = "Retry";
      retry.addEventListener("click", () => {
        // The store restarts a failed entry in place under its source.
        downloads.start(entry.source, { label: entry.label });
        toasts.show(`Download restarted: ${entry.label}`, "info");
      });
      card.append(message, retry);
      return card;
    }

    const bar = document.createElement("div");
    bar.className = "progress-bar";
    bar.setAttribute("role", "progressbar");
    bar.setAttribute("aria-label", `${entry.label} download progress`);
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    const fill = document.createElement("div");
    fill.className = "progress-bar-fill";
    fill.style.setProperty("--progress", String(entry.fraction ?? 0));
    bar.append(fill);

    const stats = document.createElement("p");
    stats.className = "download-stats";
    const percent = document.createElement("span");
    percent.className = "download-percent";
    if (entry.fraction !== null) {
      const value = Math.round(entry.fraction * 100);
      bar.setAttribute("aria-valuenow", String(value));
      percent.textContent = `${value}%`;
    } else {
      // No Content-Length: bytes downloaded stand in for the percent.
      percent.textContent = `${formatBytes(entry.bytes)} downloaded`;
    }
    stats.append(percent);
    if (entry.speedBps !== null) {
      const speed = document.createElement("span");
      speed.className = "download-speed";
      speed.textContent = `${formatBytes(entry.speedBps)}/s`;
      stats.append(speed);
    }
    if (entry.etaSeconds !== null) {
      const eta = document.createElement("span");
      eta.className = "download-eta";
      eta.textContent = `ETA ${formatEta(entry.etaSeconds)}`;
      stats.append(eta);
    }
    card.append(bar, stats);
    return card;
  };

  // ----- the Completed section ----------------------------------------------

  const renderCompleted = (): void => {
    if (!completedBox) {
      return;
    }
    if (listError !== null) {
      const banner = document.createElement("div");
      banner.className = "banner banner-danger";
      const text = document.createElement("span");
      text.textContent = `The cache listing failed: ${listError}`;
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "button button-xs button-outline";
      retry.textContent = "Retry";
      retry.addEventListener("click", () => void refreshList());
      banner.append(text, retry);
      completedBox.replaceChildren(banner);
      return;
    }
    if (completed === null) {
      const skeleton = document.createElement("div");
      skeleton.className = "skeleton-row";
      skeleton.setAttribute("aria-hidden", "true");
      completedBox.replaceChildren(skeleton);
      return;
    }
    if (completed.length === 0) {
      const empty = document.createElement("p");
      empty.className = "view-empty";
      empty.textContent = "No completed downloads in the cache.";
      completedBox.replaceChildren(empty);
      return;
    }
    const list = document.createElement("ul");
    list.className = "completed-list";
    for (const entry of completed) {
      list.append(completedRow(entry));
    }
    completedBox.replaceChildren(list);
  };

  const completedRow = (entry: CacheListEntry): HTMLElement => {
    const row = document.createElement("li");
    row.className = "completed-row";

    const check = document.createElement("span");
    check.className = "check-icon";
    check.append(lucideElement(Check, { "aria-hidden": "true", width: 16, height: 16 }));
    const done = document.createElement("span");
    done.className = "visually-hidden";
    done.textContent = "Downloaded:";
    check.append(done);

    const name = document.createElement("span");
    name.className = "model-name completed-name";
    name.textContent = filenameOf(entry);
    name.title = entry.path;

    const size = document.createElement("span");
    size.className = "completed-size";
    size.textContent = formatBytes(entry.size_bytes);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button button-xs button-outline completed-delete";
    remove.textContent = "Delete";
    remove.setAttribute("aria-label", `Delete ${filenameOf(entry)}`);
    remove.addEventListener("click", () => void deleteEntry(entry));

    row.append(check, name, size, remove);
    return row;
  };

  const deleteEntry = async (entry: CacheListEntry): Promise<void> => {
    if (!main) {
      return;
    }
    const yes = await confirmDialog(main, {
      title: "Delete cached file?",
      body: `This removes ${filenameOf(entry)} (${formatBytes(entry.size_bytes)}) from the cache.`,
      confirmLabel: "Delete",
      danger: true,
    });
    if (!yes) {
      return;
    }
    try {
      await api.deleteCached(entry.sha256);
      toasts.show(`Deleted ${filenameOf(entry)}`, "success");
    } catch (error) {
      if (error instanceof UnauthorizedError) {
        return;
      }
      toasts.show(error instanceof Error ? error.message : "The delete failed", "error");
    }
    await refreshList();
  };

  return {
    mount(target: HTMLElement): void {
      main = target;
      const title = document.createElement("h1");
      title.className = "view-title";
      title.textContent = "Downloads";

      const active = document.createElement("section");
      active.className = "downloads-section downloads-active";
      const activeHeading = document.createElement("h2");
      activeHeading.className = "downloads-heading";
      activeHeading.textContent = "Active";
      activeBox = document.createElement("div");
      active.append(activeHeading, activeBox);

      const done = document.createElement("section");
      done.className = "downloads-section downloads-completed";
      const doneHeading = document.createElement("h2");
      doneHeading.className = "downloads-heading";
      doneHeading.textContent = "Completed";
      completedBox = document.createElement("div");
      done.append(doneHeading, completedBox);

      main.replaceChildren(title, active, done);
      renderActive();
      renderCompleted();
      void refreshList();
    },
  };
}

/** The display filename of a cache entry, from its blob path. */
function filenameOf(entry: CacheListEntry): string {
  return fileName(entry.path) || entry.source;
}

/** Humanized remaining time: "45s", "2m 30s", "1h 5m". */
function formatEta(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  if (whole >= 3600) {
    return `${Math.floor(whole / 3600)}h ${Math.floor((whole % 3600) / 60)}m`;
  }
  if (whole >= 60) {
    return `${Math.floor(whole / 60)}m ${whole % 60}s`;
  }
  return `${whole}s`;
}
