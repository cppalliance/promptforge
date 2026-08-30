// Fetch wrapper for the gateway admin API. The SPA is served at
// /config/ on the gateway's own port, so the API lives one level up:
// every request goes to `..`-relative URLs with the bearer key from
// sessionStorage. A 401 from any call clears the stored key and fires
// `onUnauthorized`, which the composition root wires to the key prompt.
//
// SSE is parsed from fetch response bodies, never through EventSource:
// EventSource cannot send the Authorization header the admin routes
// require, and the switch route is a POST besides.

/** The sessionStorage slot holding the gateway bearer key. */
export const API_KEY_STORAGE_KEY = "promptforge-gateway-api-key";

/** The shape of `GET /admin/status` this UI consumes. */
export interface GatewayStatus {
  /** The active profile's name. */
  profile: string;
  /** Names of the models the running profile exposes. */
  models: string[];
}

/** The terminal outcome of a profile switch stream. */
export type SwitchResult =
  | { readonly status: "ready"; readonly profile: string }
  | { readonly status: "error"; readonly message: string };

/** The shape of `GET /admin/config-dirty`: the pending-shadow report. */
export interface DirtyReport {
  /** Whether any shadow file exists. */
  dirty: boolean;
  /** Real files whose shadows are present, relative to the config root. */
  pending_files: string[];
  /** Top-level TOML sections the pending view changes. */
  changed_sections: string[];
}

/** One unconfigured file from `GET /admin/orphans`. */
export interface OrphanFile {
  /** Cache-relative path, `/`-separated. */
  path: string;
  /** File size in bytes. */
  size_bytes: number;
  /** Cache sidecar digest; null for files the cache never downloaded. */
  sha256: string | null;
}

/** The shape of `GET /admin/model-info`: a GGUF header summary. */
export interface GgufInfo {
  /** The model architecture named by the header, when present. */
  architecture: string | null;
  /** Transformer block count, feeding the gpu_layers readout. */
  layer_count: number | null;
  /** Total parameter count, when present. */
  parameter_count: number | null;
}

/** The shape of `POST /admin/config-apply`'s reply. */
export interface ApplyOutcome {
  /** The promoted real files, relative to the config root. */
  applied: string[];
  /** Whether the active profile reloaded. */
  reloaded: boolean;
  /** Whether a promoted boot shadow needs a gateway restart. */
  restart_required: boolean;
}

/** The injectable fetch signature; tests substitute a canned gateway. */
export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/** Thrown when the gateway answers 401; the stored key is already cleared. */
export class UnauthorizedError extends Error {
  constructor() {
    super("the gateway rejected the API key");
    this.name = "UnauthorizedError";
  }
}

/** Thrown when the gateway answers a non-401 failure status. */
export class GatewayHttpError extends Error {
  /** The response's HTTP status code. */
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "GatewayHttpError";
    this.status = status;
  }
}

/** Construction dependencies for {@link GatewayApi}. */
export interface GatewayApiOptions {
  /** Transport; the browser's fetch in production, a stub in tests. */
  fetchFn: FetchLike;
  /** Where the bearer key lives; sessionStorage in production. */
  storage: Storage;
  /** API base relative to the document; the admin API is one level up. */
  base?: string;
}

/** Typed client for the admin endpoints the shell uses. */
export class GatewayApi {
  /** Fired after any 401: the stored key is gone and auth must restart. */
  onUnauthorized: (() => void) | null = null;

  /**
   * Fired with the reachability outcome of every request: true when the
   * gateway answered at all, false when the transport failed. The tab
   * bar's connection dot reflects the latest value.
   */
  onHealth: ((ok: boolean) => void) | null = null;

  private readonly fetchFn: FetchLike;
  private readonly storage: Storage;
  private readonly base: string;

  constructor(options: GatewayApiOptions) {
    this.fetchFn = options.fetchFn;
    this.storage = options.storage;
    this.base = options.base ?? "..";
  }

  /** Whether a bearer key is stored for this session. */
  hasKey(): boolean {
    return this.storage.getItem(API_KEY_STORAGE_KEY) !== null;
  }

  /** Forgets the stored bearer key. */
  clearKey(): void {
    this.storage.removeItem(API_KEY_STORAGE_KEY);
  }

  /**
   * Probes `GET /admin/status` with a candidate key. A 200 stores the
   * key and resolves true; a 401 resolves false without firing
   * `onUnauthorized`, because the caller is the key prompt itself. Any
   * other failure is thrown for the prompt to report.
   */
  async verifyKey(key: string): Promise<boolean> {
    const response = await this.transport(`${this.base}/admin/status`, {
      headers: { Authorization: `Bearer ${key}` },
    });
    if (response.status === 401) {
      return false;
    }
    if (!response.ok) {
      throw new GatewayHttpError(response.status, `the gateway answered ${response.status}`);
    }
    this.storage.setItem(API_KEY_STORAGE_KEY, key);
    return true;
  }

  /** Fetches the running profile name and model list. */
  async getStatus(): Promise<GatewayStatus> {
    const data = (await this.getJson("/admin/status")) as Partial<GatewayStatus>;
    return { profile: data.profile ?? "", models: data.models ?? [] };
  }

  /** Lists the profile names the gateway can switch to. */
  async getProfiles(): Promise<string[]> {
    const data = (await this.getJson("/admin/profiles")) as { profiles?: string[] };
    return data.profiles ?? [];
  }

  /** Fetches the running config JSON (secrets `"***"`, provenance annotated). */
  async getConfig(): Promise<Record<string, unknown>> {
    return (await this.getJson("/admin/config")) as Record<string, unknown>;
  }

  /** Fetches the pending (shadow-overlaid) config view. */
  async getConfigPending(): Promise<Record<string, unknown>> {
    const data = (await this.getJson("/admin/config-pending")) as {
      profile?: Record<string, unknown>;
    };
    return data.profile ?? {};
  }

  /** Fetches the pending-shadow dirty report. */
  async getConfigDirty(): Promise<DirtyReport> {
    const data = (await this.getJson("/admin/config-dirty")) as Partial<DirtyReport>;
    return {
      dirty: data.dirty ?? false,
      pending_files: data.pending_files ?? [],
      changed_sections: data.changed_sections ?? [],
    };
  }

  /**
   * Stages `body` (the `GET /admin/config` JSON shape) as the active
   * profile's shadow via `PUT /admin/config`. Untouched secrets stay
   * `"***"`; the gateway restores their real values.
   */
  async putConfig(body: unknown): Promise<void> {
    const response = await this.send("/admin/config", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      throw new GatewayHttpError(response.status, await refusalMessage(response));
    }
  }

  /** Promotes every shadow via `POST /admin/config-apply`. */
  async applyConfig(): Promise<ApplyOutcome> {
    const response = await this.send("/admin/config-apply", { method: "POST" });
    if (!response.ok) {
      throw new GatewayHttpError(response.status, await refusalMessage(response));
    }
    const data = (await response.json()) as Partial<ApplyOutcome>;
    return {
      applied: data.applied ?? [],
      reloaded: data.reloaded ?? false,
      restart_required: data.restart_required ?? false,
    };
  }

  /** Deletes every shadow via `POST /admin/config-revert`. */
  async revertConfig(): Promise<void> {
    const response = await this.send("/admin/config-revert", { method: "POST" });
    if (!response.ok) {
      throw new GatewayHttpError(response.status, await refusalMessage(response));
    }
  }

  /** Lists unconfigured cache files via `GET /admin/orphans`. */
  async getOrphans(): Promise<OrphanFile[]> {
    const data = (await this.getJson("/admin/orphans")) as { orphans?: OrphanFile[] };
    return data.orphans ?? [];
  }

  /**
   * Reads a GGUF header summary via `GET /admin/model-info`. Throws for
   * any failure; the caller falls back to a plain readout.
   */
  async getModelInfo(path: string): Promise<GgufInfo> {
    const data = (await this.getJson(
      `/admin/model-info?path=${encodeURIComponent(path)}`,
    )) as Partial<GgufInfo>;
    return {
      architecture: data.architecture ?? null,
      layer_count: data.layer_count ?? null,
      parameter_count: data.parameter_count ?? null,
    };
  }

  /** Opens the OS file manager at `path` via `POST /admin/reveal`. */
  async reveal(path: string): Promise<void> {
    const response = await this.send("/admin/reveal", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ path }),
    });
    if (!response.ok) {
      throw new GatewayHttpError(response.status, await refusalMessage(response));
    }
  }

  /** Deletes a cached artifact via `DELETE /v1/cache/{sha256}`. */
  async deleteCached(sha256: string): Promise<void> {
    const response = await this.send(`/v1/cache/${sha256}`, { method: "DELETE" });
    if (!response.ok) {
      throw new GatewayHttpError(response.status, await refusalMessage(response));
    }
  }

  /**
   * Runs `POST /admin/switch-profile` and consumes its SSE stream:
   * `onStage` fires for each `{"stage": ...}` marker in execution
   * order, and the returned promise settles on the terminal event -
   * `ready`, a terminal `error`, a buffered (non-SSE) refusal, or a
   * stream that ends without a terminal event, mirroring how the
   * workshop consumes the same stream.
   */
  async switchProfile(name: string, onStage: (stage: string) => void): Promise<SwitchResult> {
    const response = await this.send("/admin/switch-profile", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    const contentType = response.headers.get("content-type") ?? "";
    if (!contentType.includes("text/event-stream") || response.body === null) {
      return { status: "error", message: await refusalMessage(response) };
    }
    for await (const payload of ssePayloads(response.body)) {
      let event: Record<string, unknown>;
      try {
        event = JSON.parse(payload) as Record<string, unknown>;
      } catch {
        continue;
      }
      if (typeof event["stage"] === "string") {
        onStage(event["stage"]);
      } else if (event["status"] === "ready") {
        const profile = typeof event["profile"] === "string" ? event["profile"] : name;
        return { status: "ready", profile };
      } else if (event["status"] === "error") {
        const message =
          typeof event["message"] === "string" ? event["message"] : "the switch failed";
        return { status: "error", message };
      }
    }
    return { status: "error", message: "the switch stream ended without a terminal event" };
  }

  /**
   * Subscribes to the `GET /admin/progress` SSE stream, invoking
   * `onEvent` with each parsed progress event. Returns the unsubscribe
   * function. A transport failure ends the subscription quietly; a 401
   * flows through the shared unauthorized path.
   */
  subscribeProgress(onEvent: (event: unknown) => void): () => void {
    const controller = new AbortController();
    void (async () => {
      let response: Response;
      try {
        response = await this.send("/admin/progress", { signal: controller.signal });
      } catch {
        // Unreachable gateway or 401; onHealth/onUnauthorized already fired.
        return;
      }
      if (!response.ok || response.body === null) {
        return;
      }
      try {
        for await (const payload of ssePayloads(response.body)) {
          try {
            onEvent(JSON.parse(payload));
          } catch {
            // A malformed event is dropped; the stream carries on.
          }
        }
      } catch {
        // The stream broke mid-flight (or was aborted); later requests
        // will report reachability through onHealth.
      }
    })();
    return () => controller.abort();
  }

  /** GETs a JSON endpoint, throwing on any non-success status. */
  private async getJson(path: string): Promise<unknown> {
    const response = await this.send(path);
    if (!response.ok) {
      throw new GatewayHttpError(response.status, `the gateway answered ${response.status}`);
    }
    return response.json();
  }

  /**
   * Sends one authenticated request. A 401 clears the key, fires
   * `onUnauthorized`, and throws {@link UnauthorizedError}.
   */
  private async send(path: string, init: RequestInit = {}): Promise<Response> {
    const key = this.storage.getItem(API_KEY_STORAGE_KEY) ?? "";
    const headers: Record<string, string> = {
      Authorization: `Bearer ${key}`,
      ...((init.headers as Record<string, string> | undefined) ?? {}),
    };
    const response = await this.transport(this.base + path, { ...init, headers });
    if (response.status === 401) {
      this.clearKey();
      this.onUnauthorized?.();
      throw new UnauthorizedError();
    }
    return response;
  }

  /** Runs the raw fetch and reports reachability to `onHealth`. */
  private async transport(url: string, init: RequestInit): Promise<Response> {
    let response: Response;
    try {
      response = await this.fetchFn(url, init);
    } catch (error) {
      this.onHealth?.(false);
      throw error;
    }
    this.onHealth?.(true);
    return response;
  }
}

/** Extracts a human-readable message from a buffered refusal response. */
async function refusalMessage(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as Record<string, unknown>;
    const error = body["error"];
    if (typeof error === "string") {
      return error;
    }
    if (error !== null && typeof error === "object") {
      const message = (error as Record<string, unknown>)["message"];
      if (typeof message === "string") {
        return message;
      }
    }
  } catch {
    // Fall through to the status line.
  }
  return `the gateway refused the switch (${response.status})`;
}

/**
 * Yields each SSE event's data payload from a streaming response body.
 * Multi-line data fields join with newlines per the SSE spec; comment
 * and non-data lines (the progress stream's heartbeats) are skipped.
 * Early generator exit cancels the reader, closing the connection.
 */
async function* ssePayloads(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let data: string[] = [];
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      buffer += decoder.decode(value, { stream: true });
      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        let line = buffer.slice(0, newline);
        buffer = buffer.slice(newline + 1);
        if (line.endsWith("\r")) {
          line = line.slice(0, -1);
        }
        if (line === "") {
          if (data.length > 0) {
            yield data.join("\n");
            data = [];
          }
        } else if (line.startsWith("data:")) {
          data.push(line.slice(5).replace(/^ /, ""));
        }
        newline = buffer.indexOf("\n");
      }
    }
    if (data.length > 0) {
      yield data.join("\n");
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
}
