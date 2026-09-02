// Desktop update state and I/O. The service is DOM-free: views subscribe to
// snapshots and the composition root owns its lifetime.

import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";

import { Emitter, type Event } from "../base/event";
import { Disposable, toDisposable } from "../base/lifecycle";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "dismissed"
  | "downloading"
  | "installing"
  | "restarting"
  | "error"
  | "browser";

export interface UpdateSnapshot {
  readonly phase: UpdatePhase;
  readonly currentVersion: string;
  readonly version: string;
  readonly notes: string;
  readonly downloaded: number;
  readonly total: number | null;
  readonly error: string;
  readonly log: readonly string[];
}

interface DesktopUpdate {
  readonly currentVersion: string;
  readonly version: string;
  readonly body?: string;
  downloadAndInstall(onEvent?: (event: DownloadEvent) => void): Promise<void>;
  close(): Promise<void>;
}

export interface UpdateBackend {
  readonly desktop: boolean;
  currentVersion(): Promise<string>;
  check(): Promise<DesktopUpdate | null>;
  relaunch(): Promise<void>;
}

function defaultBackend(): UpdateBackend {
  return {
    desktop: typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined,
    currentVersion: getVersion,
    check: async (): Promise<Update | null> => check({ timeout: 30_000 }),
    relaunch,
  };
}

const EMPTY: UpdateSnapshot = {
  phase: "idle",
  currentVersion: "",
  version: "",
  notes: "",
  downloaded: 0,
  total: null,
  error: "",
  log: [],
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function oneLine(text: string | undefined): string {
  return (text ?? "").split(/\r?\n/, 1)[0]?.trim() ?? "";
}

export class UpdateService extends Disposable {
  private readonly changes = this._register(new Emitter<UpdateSnapshot>());
  private state: UpdateSnapshot = EMPTY;
  private update: DesktopUpdate | null = null;
  private checking: Promise<void> | null = null;

  readonly onDidChange: Event<UpdateSnapshot> = this.changes.event;

  constructor(private readonly backend: UpdateBackend = defaultBackend()) {
    super();
    if (!backend.desktop) {
      this.state = { ...EMPTY, phase: "browser" };
    }
  }

  get snapshot(): UpdateSnapshot {
    return this.state;
  }

  startAutoCheck(delayMs = 5_000): void {
    if (!this.backend.desktop) {
      return;
    }
    const timer = window.setTimeout(() => void this.checkNow(), delayMs);
    this._register(toDisposable(() => window.clearTimeout(timer)));
  }

  async checkNow(): Promise<void> {
    if (!this.backend.desktop) {
      this.publish({ phase: "browser", error: "" });
      return;
    }
    if (this.checking !== null) {
      return this.checking;
    }
    this.checking = this.runCheck();
    try {
      await this.checking;
    } finally {
      this.checking = null;
    }
  }

  remindLater(): void {
    if (this.update !== null) {
      this.publish({ phase: "dismissed" });
    }
  }

  showAvailable(): void {
    if (this.update !== null) {
      this.publish({ phase: "available" });
    }
  }

  async install(): Promise<void> {
    const update = this.update;
    if (update === null) {
      return;
    }
    let downloaded = 0;
    this.publish({
      phase: "downloading",
      downloaded,
      total: null,
      error: "",
      log: [...this.state.log, `Downloading PromptForge ${update.version}`],
    });
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          this.publish({
            phase: "downloading",
            total: event.data.contentLength ?? null,
          });
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          this.publish({ phase: "downloading", downloaded });
        } else {
          this.publish({
            phase: "installing",
            log: [...this.state.log, "Download complete. Installing update."],
          });
        }
      });
      this.publish({
        phase: "restarting",
        log: [...this.state.log, "Update installed. Restarting PromptForge."],
      });
      await this.backend.relaunch();
    } catch (error) {
      const message = errorText(error);
      this.publish({
        phase: "error",
        error: message,
        log: [...this.state.log, `Update failed: ${message}`],
      });
    }
  }

  override dispose(): void {
    if (this.update !== null) {
      void this.update.close();
      this.update = null;
    }
    super.dispose();
  }

  private async runCheck(): Promise<void> {
    this.publish({ phase: "checking", error: "" });
    try {
      const currentVersion = await this.backend.currentVersion();
      const update = await this.backend.check();
      if (this.update !== null && this.update !== update) {
        await this.update.close();
      }
      this.update = update;
      if (update === null) {
        this.publish({
          phase: "current",
          currentVersion,
          version: "",
          notes: "",
          downloaded: 0,
          total: null,
          log: [],
        });
        return;
      }
      this.publish({
        phase: "available",
        currentVersion: update.currentVersion || currentVersion,
        version: update.version,
        notes: oneLine(update.body),
        downloaded: 0,
        total: null,
        log: [`PromptForge ${update.version} is available.`],
      });
    } catch (error) {
      const message = errorText(error);
      this.publish({
        phase: "error",
        error: message,
        log: [`Update check failed: ${message}`],
      });
    }
  }

  private publish(change: Partial<UpdateSnapshot>): void {
    this.state = { ...this.state, ...change };
    this.changes.fire(this.state);
  }
}
