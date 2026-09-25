// The cloud model sheet store: holds the gateway's cached cloud
// provider model sheet client-side, mirroring the ConfigStore shape
// (subscribe/notify; views subscribe on mount and re-render on change).
// `start` requests GET /admin/cloud-models and, while the gateway
// answers its 503 loading indication (no cache yet, download in
// flight), polls at a short interval until the sheet arrives or the
// gateway reports the download error, then notifies. `refresh` forces
// a re-download whose POST answer returns the fresh sheet, stored and
// notified directly.

import { GatewayHttpError } from "./gateway-api";
import type { CloudSheet, GatewayApi } from "./gateway-api";

/** The store's lifecycle: polling, holding a sheet, or failed. */
export type SheetStatus = "loading" | "loaded" | "error";

/** The default poll interval while the gateway downloads the sheet. */
const DEFAULT_POLL_MS = 1_000;

/** The subscribable cloud sheet store; one per desk mount. */
export class SheetStore {
  /** The current lifecycle state. */
  status: SheetStatus = "loading";
  /** The loaded sheet, when one has arrived. */
  sheet: CloudSheet | null = null;
  /** The terminal failure message, in the error state. */
  error: string | null = null;

  private readonly api: GatewayApi;
  private readonly pollMs: number;
  private readonly listeners = new Set<() => void>();
  /** Invalidates in-flight poll cycles on restart and dispose. */
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(api: GatewayApi, pollMs: number = DEFAULT_POLL_MS) {
    this.api = api;
    this.pollMs = pollMs;
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

  /** (Re)starts the request cycle from the loading state. */
  start(): void {
    const generation = ++this.generation;
    this.status = "loading";
    this.error = null;
    this.notify();
    void this.poll(generation);
  }

  /**
   * Forces a re-download regardless of cache age; the POST answer
   * returns the fresh sheet, which is stored and notified directly. A
   * failed re-download request records the error state, notifies, and
   * rejects so the caller can surface the failure.
   */
  async refresh(): Promise<void> {
    const generation = ++this.generation;
    try {
      const sheet = await this.api.refreshCloudModels();
      if (generation !== this.generation) {
        return;
      }
      this.sheet = sheet;
      this.status = "loaded";
      this.error = null;
      this.notify();
    } catch (error) {
      if (generation !== this.generation) {
        return;
      }
      this.status = "error";
      this.error = error instanceof Error ? error.message : String(error);
      this.notify();
      throw error;
    }
  }

  /** Stops polling; the desk's teardown calls this. */
  dispose(): void {
    this.generation += 1;
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  /** One request; schedules the next while the gateway says loading. */
  private async poll(generation: number): Promise<void> {
    try {
      const sheet = await this.api.getCloudModels();
      if (generation !== this.generation) {
        return;
      }
      this.sheet = sheet;
      this.status = "loaded";
      this.error = null;
      this.notify();
    } catch (error) {
      if (generation !== this.generation) {
        return;
      }
      if (error instanceof GatewayHttpError && error.code === "cloud_models_loading") {
        this.timer = setTimeout(() => void this.poll(generation), this.pollMs);
        return;
      }
      this.status = "error";
      this.error = error instanceof Error ? error.message : String(error);
      this.notify();
    }
  }
}
