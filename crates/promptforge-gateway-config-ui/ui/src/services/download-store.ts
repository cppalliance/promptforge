// The global download store: state for every `POST /v1/cache` download
// this session started, keyed by source URL. It lives at the shell
// level so downloads survive view navigation; the top progress strip
// and (later) the Downloads view subscribe to it. The gateway offers no
// cancel endpoint - a dropped stream leaves the server-side download
// running - so the store exposes none.

import type { CacheProgressSample, GatewayApi } from "./gateway-api";

/** A download's lifecycle state. */
export type DownloadStatus = "downloading" | "ready" | "error";

/** One tracked download. */
export interface DownloadEntry {
  /** The source URL handed to `POST /v1/cache` (the store key). */
  readonly source: string;
  /** Display label (the filename). */
  readonly label: string;
  /** Lifecycle state. */
  status: DownloadStatus;
  /** Bytes downloaded so far. */
  bytes: number;
  /** Total bytes, or null when the server sent no Content-Length. */
  total: number | null;
  /** 0-1 completion, or null while the total is unknown. */
  fraction: number | null;
  /** Smoothed download speed in bytes per second, while downloading. */
  speedBps: number | null;
  /** Estimated seconds remaining, when speed and total are known. */
  etaSeconds: number | null;
  /** The cached blob's path, once ready. */
  path: string | null;
  /** The failure message, on error. */
  message: string | null;
}

/** Weight of the newest sample in the speed's moving average. */
const SPEED_SMOOTHING = 0.3;

/** The store. Constructed once per shell mount. */
export class DownloadStore {
  private readonly api: GatewayApi;
  private readonly now: () => number;
  private readonly downloads = new Map<string, DownloadEntry>();
  private readonly listeners = new Set<() => void>();

  constructor(api: GatewayApi, now: () => number = () => Date.now()) {
    this.api = api;
    this.now = now;
  }

  /** Registers a change listener; returns the unsubscribe function. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const listener of this.listeners) {
      listener();
    }
  }

  /** Every tracked download, in start order. */
  entries(): DownloadEntry[] {
    return [...this.downloads.values()];
  }

  /** The downloads still in flight. */
  active(): DownloadEntry[] {
    return this.entries().filter((entry) => entry.status === "downloading");
  }

  /** Whether a download for `source` is in flight. */
  isActive(source: string): boolean {
    return this.downloads.get(source)?.status === "downloading";
  }

  /**
   * The mean completion fraction across active downloads, for the top
   * strip; an unknown-total download counts as 0, and no active
   * download yields 0.
   */
  overallFraction(): number {
    const active = this.active();
    if (active.length === 0) {
      return 0;
    }
    const sum = active.reduce((total, entry) => total + (entry.fraction ?? 0), 0);
    return sum / active.length;
  }

  /**
   * Starts a download unless one for `source` is already in flight.
   * A finished (ready or failed) entry restarts in place, so a retry
   * after an error is one more click.
   */
  start(source: string, options: { label?: string; sha256?: string | null } = {}): void {
    if (this.isActive(source)) {
      return;
    }
    const entry: DownloadEntry = {
      source,
      label: options.label ?? source.split("/").pop() ?? source,
      status: "downloading",
      bytes: 0,
      total: null,
      fraction: null,
      speedBps: null,
      etaSeconds: null,
      path: null,
      message: null,
    };
    this.downloads.set(source, entry);
    this.notify();

    let lastBytes = 0;
    let lastTime = this.now();
    void this.api
      .cacheDownload(source, options.sha256 ?? null, (sample: CacheProgressSample) => {
        const time = this.now();
        const deltaSeconds = (time - lastTime) / 1000;
        const deltaBytes = sample.bytes - lastBytes;
        if (deltaSeconds > 0 && deltaBytes >= 0) {
          const instant = deltaBytes / deltaSeconds;
          entry.speedBps =
            entry.speedBps === null
              ? instant
              : entry.speedBps * (1 - SPEED_SMOOTHING) + instant * SPEED_SMOOTHING;
        }
        lastBytes = sample.bytes;
        lastTime = time;
        entry.bytes = sample.bytes;
        entry.total = sample.total;
        entry.fraction =
          sample.total !== null && sample.total > 0
            ? Math.min(sample.bytes / sample.total, 1)
            : null;
        entry.etaSeconds =
          sample.total !== null && entry.speedBps !== null && entry.speedBps > 0
            ? Math.max(sample.total - sample.bytes, 0) / entry.speedBps
            : null;
        this.notify();
      })
      .then((outcome) => {
        if (outcome.status === "ready") {
          entry.status = "ready";
          entry.path = outcome.path;
          entry.fraction = 1;
          if (entry.total !== null) {
            entry.bytes = entry.total;
          }
        } else {
          entry.status = "error";
          entry.message = outcome.message;
        }
        entry.speedBps = null;
        entry.etaSeconds = null;
        this.notify();
      })
      .catch((error: unknown) => {
        // Unreachable gateway or a 401 (the key prompt already took
        // over); the entry records the failure for the Downloads view.
        entry.status = "error";
        entry.message = error instanceof Error ? error.message : String(error);
        entry.speedBps = null;
        entry.etaSeconds = null;
        this.notify();
      });
  }
}
