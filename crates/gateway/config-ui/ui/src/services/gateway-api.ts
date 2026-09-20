// Fetch wrapper for the gateway admin API. The SPA is served at
// /config/ on the gateway's own port, so the API lives one level up:
// every request goes to `..`-relative URLs with the bearer key from
// sessionStorage. A 401 from any call clears the stored key and fires
// `onUnauthorized`, which the composition root wires to the key prompt.
//
// SSE is parsed from fetch response bodies, never through EventSource:
// EventSource cannot send the Authorization header the admin routes
// require.

import { isRecord } from "./json";

/** The sessionStorage slot holding the gateway bearer key. */
export const API_KEY_STORAGE_KEY = "gateway-api-key";

/** One capability endpoint's readiness, from `GET /admin/status`. */
export interface EndpointStatus {
  /** The route path, for example `/v1/chat/completions`. */
  path: string;
  /** The display name, for example `Chat completions`. */
  name: string;
  /** Whether at least one loaded model serves the endpoint. */
  ready: boolean;
  /** Whether a queue command is provisioning the endpoint's models. */
  provisioning: boolean;
}

/**
 * The gateway's live activity, from `GET /admin/status` and each
 * `GET /admin/progress` frame: a busy flag and the newest activity's text.
 */
export interface Progress {
  /** Whether any activity is live; the UIs show a barberpole while set. */
  busy: boolean;
  /** The newest live activity's text, e.g. `Downloading qwen 45%`; empty idle. */
  text: string;
}

/** The command queue readout from `GET /admin/status`. */
export interface QueueStatus {
  /** The running command, or null when the worker is idle. */
  active: {
    /** The command's display name, for example `load-profile: main`. */
    name: string;
    /** When the worker started the command, in Unix epoch seconds. */
    started_at: number;
  } | null;
  /** The waiting commands, oldest first. */
  pending: Array<{
    /** The command's display name. */
    name: string;
    /** When the command entered the queue, in Unix epoch seconds. */
    queued_at: number;
  }>;
}

/** The shape of `GET /admin/status` this UI consumes. */
export interface GatewayStatus {
  /** The running profile's name; empty when no profile is running. */
  profile: string;
  /** Names of the models the running profile exposes. */
  models: string[];
  /** Process-lifetime config generation, changed by a gateway restart. */
  config_generation: string;
  /** Declared VRAM total of the active local and STT models, in GiB. */
  vram_gb: number;
  /** The hub's current activity snapshot. */
  progress: Progress;
  /** The command queue's active and pending commands. */
  queue: QueueStatus;
  /** One readiness entry per capability endpoint the gateway can serve. */
  endpoints: EndpointStatus[];
}

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

/** One bundled chat-template family exposed by the gateway catalog. */
export interface ChatTemplateFamily {
  /** Stable value written after `builtin:`. */
  slug: string;
  /** Operator-facing dropdown label. */
  label: string;
}

/** The effective source categories shared with launch-time resolution. */
export type ChatTemplateSource = "embedded" | "known-override" | "builtin" | "custom";

/** One local chat model's launch-time template decision. */
export interface ChatTemplateModelResolution {
  /** Configured local model name. */
  name: string;
  /** Effective source selected by launch precedence. */
  effective_source: ChatTemplateSource;
  /** Bundled family selected by an override or explicit choice. */
  effective_family: string | null;
  /** Family detected by the exact server-side model mapper. */
  detected_family: string | null;
  /** Operator-facing reason for the decision. */
  reason: string;
}

/** One exact Hugging Face model ID mapping used by Discover. */
export interface ChatTemplateMapping {
  /** Exact lowercase repository identifier. */
  model_id: string;
  /** Bundled family slug. */
  family: string;
}

/** The validated `GET /admin/chat-templates` response. */
export interface ChatTemplateCatalog {
  /** Bundled families in dropdown order. */
  families: ChatTemplateFamily[];
  /** Exact model mappings from the Rust catalog. */
  mappings: ChatTemplateMapping[];
  /** Effective decisions for configured local chat models. */
  models: ChatTemplateModelResolution[];
}

/** The `GET /admin/system` snapshot the Settings and Discover views consume. */
export interface SystemSnapshot {
  /** Processor identity and load; null when the reply omits it. */
  cpu: {
    frequency_mhz: number;
    logical_cores: number;
    physical_cores: number | null;
    utilization_percent: number;
  } | null;
  /** Physical memory usage in bytes. */
  ram: { used_bytes: number; total_bytes: number };
  /** Usage of the drive holding the artifact cache; null when unresolved. */
  disk: { cache_dir: string; used_bytes: number; total_bytes: number } | null;
  /** The first NVIDIA GPU; null on machines without an NVML driver. */
  gpu: { name: string; vram_used_bytes: number; vram_total_bytes: number } | null;
}

/**
 * One cached blob from `GET /v1/cache`. The sidecar records no
 * timestamp, so the listing carries no download date.
 */
export interface CacheListEntry {
  /** The URL the blob was downloaded from. */
  source: string;
  /** Absolute path of the blob under the cache root. */
  path: string;
  /** Lowercase hex SHA-256 of the blob's bytes. */
  sha256: string;
  /** Blob length in bytes. */
  size_bytes: number;
}

/** The shape of `POST /admin/config-apply`'s reply. */
export interface ApplyOutcome {
  /** The promoted real files, relative to the config root. */
  applied: string[];
  /** Whether remote routing reloaded live. */
  reloaded: boolean;
  /** Whether a promoted change needs a gateway restart to take effect. */
  restart_required: boolean;
}

/** The shape of `POST /admin/switch-profile`'s reply. */
export interface SwitchOutcome {
  /** The persisted selection; null when no profile is selected. */
  profile: string | null;
  /** Whether the selection differs from the running profile. */
  restart_required: boolean;
}

/** The `GET /admin/config-pending` envelope this UI consumes. */
export interface PendingView {
  /** The shadow-preferred global config, secrets redacted. */
  config: Record<string, unknown>;
  /**
   * The persisted selection from `gateway.state.toml`: null when none is
   * persisted, otherwise the raw name even when the config no longer
   * defines it.
   */
  activeProfile: string | null;
}

/** One environment variable a cloud provider reads, from the sheet. */
export interface CloudEnvVar {
  /** The variable name, for example `ANTHROPIC_API_KEY`. */
  name: string;
  /** `key` for credential material, `config` for regions and endpoints. */
  role: "key" | "config";
  /** The value the provider assumes when the variable is unset. */
  default: string | null;
}

/** The sheet's reasoning capability triple for one model. */
export interface CloudModelThinking {
  /** Any reasoning capability at all. */
  supported: boolean;
  /** Manual budget mode (a per-call switch). */
  enabled: boolean;
  /** Model-chosen thinking depth. */
  adaptive: boolean;
}

/** Per-million-token pricing, from the sheet. */
export interface CloudModelPricing {
  /** ISO 4217, for example `USD`. */
  currency: string;
  /** Prompt price per million tokens. */
  prompt_per_mtok: number;
  /** Completion price per million tokens. */
  completion_per_mtok: number;
}

/** One cloud model entry from `GET /admin/cloud-models`. */
export interface CloudModelEntry {
  /** The upstream slug. */
  id: string;
  /** UI-facing name. */
  display_name: string;
  /** The UI's first grouping level. */
  family: string;
  /** The canonical alias this entry is a variant of, when one exists. */
  variant_of: string | null;
  /** The stripped suffix, for the expander label. */
  variant: string | null;
  /** The provider's own language codes. */
  languages: string[];
  /** The workload: chat, image, video, transcription, speech, ... */
  kind: string;
  /** Context window in tokens; null for the IDs-only providers. */
  context_window: number | null;
  /** Maximum completion tokens, when reported. */
  max_output: number | null;
  /** Accepts image input. */
  images: boolean;
  /** Emits tool calls. */
  tool_calling: boolean;
  /** Reasoning capability. */
  thinking: CloudModelThinking;
  /** The provider's own effort level names. */
  effort_levels: string[];
  /** The provider's own default level name. */
  default_effort: string | null;
  /** Per-million-token pricing, when reported. */
  pricing: CloudModelPricing | null;
}

/** One provider's slice of the cloud model sheet. */
export interface CloudProviderSlice {
  /** UI-facing name, for example `Anthropic`. */
  display_name: string;
  /** The curated tier: prime, subprime, niche, aggregator. */
  tier: string;
  /** Freshness of this slice. */
  status: string;
  /** The base URL of the provider's OpenAI-compatible chat API, if any. */
  openai_base_url: string | null;
  /** The provider's environment variables. */
  env_vars: CloudEnvVar[];
  /** The provider's normalized model entries. */
  models: CloudModelEntry[];
}

/** The cloud provider model sheet from `GET /admin/cloud-models`. */
export interface CloudSheet {
  /** The sheet schema version. */
  schema_version: number;
  /** RFC 3339 generation time; the cache age source. */
  generated_at: string;
  /** Provider slices keyed by provider name. */
  providers: Record<string, CloudProviderSlice>;
}

/** One side of `GET /admin/env`: an env file's path and parsed variables. */
export interface EnvSide {
  /** The `.env` file's absolute path. */
  path: string;
  /** The parsed variables, values included (the route is key-guarded). */
  vars: Record<string, string>;
}

/** The environment data consumed by the single-file Secrets view. */
export interface EnvFiles {
  /** The config's sibling (`gateway.env`). */
  global: EnvSide | null;
  /**
   * Each `${VAR}` name the pending config chain references, mapped to
   * labels of the referencing fields (`endpoint openai api_key`).
   * Computed server-side from the raw pre-interpolation chain - the
   * client never sees references, because the config views arrive
   * interpolated with secrets redacted.
   */
  references: Record<string, string[]>;
}

/** The single environment-file write scope. */
export type EnvScope = "global";

/** The injectable fetch signature; tests substitute a canned gateway. */
export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/** Thrown when the gateway answers 401; the stored key is already cleared. */
export class UnauthorizedError extends Error {
  constructor() {
    super("the gateway rejected the API key");
    this.name = "UnauthorizedError";
  }
}

/**
 * Thrown when the Hugging Face hub refused a proxied call with 401: no
 * HF_TOKEN is configured (or the configured one is invalid). Distinct
 * from {@link UnauthorizedError}, which is the gateway refusing the
 * SPA's own bearer key.
 */
export class HfAuthError extends Error {
  constructor() {
    super("the Hugging Face hub rejected the request; set HF_TOKEN");
    this.name = "HfAuthError";
  }
}

/** Thrown when the gateway answers a non-401 failure status. */
export class GatewayHttpError extends Error {
  /** The response's HTTP status code. */
  readonly status: number;
  /** The envelope's `error.code` (`apply_cancelled`, ...), when it sent one. */
  readonly code: string | null;

  constructor(status: number, message: string, code: string | null = null) {
    super(message);
    this.name = "GatewayHttpError";
    this.status = status;
    this.code = code;
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
   * Probes whether an ambient credential authenticates this session. The
   * `/auth` browser handoff lands on the SPA with an HttpOnly cookie and
   * no stored key; a 200 from `GET /admin/status` with no presented key
   * means the cookie carried auth, and anything else means the key
   * prompt. Never fires `onUnauthorized` and never throws: a failed probe
   * simply resolves false.
   */
  async hasAmbientAuth(): Promise<boolean> {
    try {
      const response = await this.transport(`${this.base}/admin/status`, {});
      return response.ok;
    } catch {
      return false;
    }
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

  /** Fetches the running profile name, model list, queue, and endpoints. */
  async getStatus(): Promise<GatewayStatus> {
    const raw = await this.getJson("/admin/status");
    const data = isRecord(raw) ? raw : {};
    const queue = isRecord(data["queue"]) ? data["queue"] : {};
    const active = isRecord(queue["active"]) ? queue["active"] : null;
    return {
      profile: typeof data["profile"] === "string" ? data["profile"] : "",
      models: Array.isArray(data["models"])
        ? data["models"].filter((model): model is string => typeof model === "string")
        : [],
      config_generation:
        typeof data["config_generation"] === "string" ? data["config_generation"] : "",
      vram_gb: numberOrZero(data["vram_gb"]),
      progress: parseProgress(data["progress"]),
      queue: {
        active:
          active !== null && typeof active["name"] === "string"
            ? {
                name: active["name"],
                started_at: numberOrZero(active["started_at"]),
              }
            : null,
        pending: Array.isArray(queue["pending"])
          ? queue["pending"].flatMap((entry) =>
              isRecord(entry) && typeof entry["name"] === "string"
                ? [{ name: entry["name"], queued_at: numberOrZero(entry["queued_at"]) }]
                : [],
            )
          : [],
      },
      endpoints: Array.isArray(data["endpoints"])
        ? data["endpoints"].flatMap((entry) =>
            isRecord(entry) &&
            typeof entry["path"] === "string" &&
            typeof entry["name"] === "string"
              ? [
                  {
                    path: entry["path"],
                    name: entry["name"],
                    ready: entry["ready"] === true,
                    provisioning: entry["provisioning"] === true,
                  },
                ]
              : [],
          )
        : [],
    };
  }

  /**
   * Fires the active command's cancellation token via
   * `POST /admin/queue/cancel`. The command settles as cancelled at its
   * next chunk or phase boundary; the next status poll reflects it.
   */
  async cancelActiveCommand(): Promise<void> {
    const response = await this.send("/admin/queue/cancel", { method: "POST" });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /**
   * Removes the waiting command at `index` via
   * `POST /admin/queue/cancel-pending`; its waiters settle as cancelled.
   */
  async cancelPendingCommand(index: number): Promise<void> {
    const response = await this.send("/admin/queue/cancel-pending", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ index }),
    });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /** Fetches the running config JSON (secrets `"***"`, provenance annotated). */
  async getConfig(): Promise<Record<string, unknown>> {
    return requireRecord(await this.getJson("/admin/config"), "config");
  }

  /**
   * Fetches the pending (shadow-overlaid) config view. The envelope's
   * `profile` object carries the persisted selection as `active_profile`;
   * it is split out here because it is not a configuration key and must
   * never ride back in a `PUT /admin/config` body.
   */
  async getConfigPending(): Promise<PendingView> {
    const data = requireRecord(await this.getJson("/admin/config-pending"), "pending config");
    if (data["profile"] === undefined) {
      return { config: {}, activeProfile: null };
    }
    const { active_profile: selection, ...config } = requireRecord(
      data["profile"],
      "pending config profile",
    );
    return {
      config,
      activeProfile: typeof selection === "string" ? selection : null,
    };
  }

  /** Fetches the pending-shadow dirty report. */
  async getConfigDirty(): Promise<DirtyReport> {
    const data = requireRecord(await this.getJson("/admin/config-dirty"), "dirty report");
    return {
      dirty: data["dirty"] === true,
      pending_files: stringArray(data["pending_files"]),
      changed_sections: stringArray(data["changed_sections"]),
    };
  }

  /**
   * Stages `body` (the `GET /admin/config` JSON shape) as the config
   * shadow via `PUT /admin/config`. Untouched secrets stay `"***"`; the
   * gateway restores their real values. A body carrying `active_profile`
   * is refused: selection belongs to {@link switchProfile}.
   */
  async putConfig(body: unknown): Promise<void> {
    const response = await this.send("/admin/config", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /** Promotes every shadow via `POST /admin/config-apply`. */
  async applyConfig(): Promise<ApplyOutcome> {
    const response = await this.send("/admin/config-apply", { method: "POST" });
    if (!response.ok) {
      // The code distinguishes a cancelled apply (pending changes still
      // staged) from a failed one, so the shell can word its toast.
      throw await refusalError(response);
    }
    const data = requireRecord(await response.json(), "apply outcome");
    return {
      applied: stringArray(data["applied"]),
      reloaded: data["reloaded"] === true,
      restart_required: data["restart_required"] === true,
    };
  }

  /**
   * Persists the profile selection via `POST /admin/switch-profile`:
   * a name writes `gateway.state.toml`, `null` clears it. The running
   * profile never changes; the reply's `restart_required` says whether a
   * restart is needed for the selection to run.
   */
  async switchProfile(name: string | null): Promise<SwitchOutcome> {
    const response = await this.send("/admin/switch-profile", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    if (!response.ok) {
      throw await refusalError(response);
    }
    const data = requireRecord(await response.json(), "switch outcome");
    return {
      profile: typeof data["profile"] === "string" ? data["profile"] : null,
      restart_required: data["restart_required"] === true,
    };
  }

  /** Deletes every shadow via `POST /admin/config-revert`. */
  async revertConfig(): Promise<void> {
    const response = await this.send("/admin/config-revert", { method: "POST" });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /**
   * Reads the global `.env` file via `GET /admin/env`. Values arrive in
   * plaintext (the route is loopback-and-bearer-guarded); the caller
   * masks them in the DOM and never logs them.
   */
  async getEnv(signal?: AbortSignal): Promise<EnvFiles> {
    const data = requireRecord(await this.getJson("/admin/env", signal), "environment files");
    const side = (raw: unknown): EnvSide | null => {
      if (raw === null || raw === undefined) {
        return null;
      }
      const value = requireRecord(raw, "environment file");
      return {
        path: typeof value["path"] === "string" ? value["path"] : "",
        vars: stringRecord(value["vars"]),
      };
    };
    const references: Record<string, string[]> = {};
    if (isRecord(data["references"])) {
      for (const [key, value] of Object.entries(data["references"])) {
        references[key] = stringArray(value);
      }
    }
    return {
      global: side(data["boot"]),
      references,
    };
  }

  /**
   * Stages `vars` as the global `.env.next` shadow via `PUT /admin/env`.
   * The real file is untouched until Apply, and the process reads the
   * promoted values after restart.
   */
  async putEnv(vars: Record<string, string>): Promise<void> {
    const response = await this.send("/admin/env", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(vars),
    });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /** Lists unconfigured cache files via `GET /admin/orphans`. */
  async getOrphans(): Promise<OrphanFile[]> {
    const raw = await this.getJson("/admin/orphans");
    if (!isRecord(raw) || !Array.isArray(raw["orphans"])) {
      return [];
    }
    return raw["orphans"].flatMap((item) => {
      if (
        !isRecord(item) ||
        typeof item["path"] !== "string" ||
        typeof item["size_bytes"] !== "number"
      ) {
        return [];
      }
      const sha256 = item["sha256"];
      return [
        {
          path: item["path"],
          size_bytes: item["size_bytes"],
          sha256:
            typeof sha256 === "string" && /^[0-9a-f]{64}$/i.test(sha256)
              ? sha256.toLowerCase()
              : null,
        },
      ];
    });
  }

  /**
   * Reads a GGUF header summary via `GET /admin/model-info`. Throws for
   * any failure; the caller falls back to a plain readout.
   */
  async getModelInfo(path: string): Promise<GgufInfo> {
    const data = requireRecord(
      await this.getJson(`/admin/model-info?path=${encodeURIComponent(path)}`),
      "model info",
    );
    return {
      architecture: typeof data["architecture"] === "string" ? data["architecture"] : null,
      layer_count: typeof data["layer_count"] === "number" ? data["layer_count"] : null,
      parameter_count:
        typeof data["parameter_count"] === "number" ? data["parameter_count"] : null,
    };
  }

  /** Fetches and validates the local chat-template catalog. */
  async getChatTemplates(signal?: AbortSignal): Promise<ChatTemplateCatalog> {
    const data = requireRecord(
      await this.getJson("/admin/chat-templates", signal),
      "chat-template catalog",
    );
    const families = requireArray(data["families"], "chat-template families").map((raw) => {
      const family = requireRecord(raw, "chat-template family");
      return {
        slug: requireString(family["slug"], "chat-template family slug"),
        label: requireString(family["label"], "chat-template family label"),
      };
    });
    const slugs = new Set(families.map((family) => family.slug));
    const mappings = requireArray(data["mappings"], "chat-template mappings").map((raw) => {
      const mapping = requireRecord(raw, "chat-template mapping");
      const family = requireString(mapping["family"], "chat-template mapping family");
      if (!slugs.has(family)) {
        throw new TypeError(`the gateway returned unknown mapped chat-template family ${family}`);
      }
      return {
        model_id: requireString(mapping["model_id"], "chat-template model ID"),
        family,
      };
    });
    const models = requireArray(data["models"], "chat-template resolutions").map((raw) => {
      const model = requireRecord(raw, "chat-template resolution");
      const source = requireChatTemplateSource(model["effective_source"]);
      const effectiveFamily = nullableString(
        model["effective_family"],
        "effective chat-template family",
      );
      const detectedFamily = nullableString(
        model["detected_family"],
        "detected chat-template family",
      );
      if (effectiveFamily !== null && !slugs.has(effectiveFamily)) {
        throw new TypeError(
          `the gateway returned unknown effective chat-template family ${effectiveFamily}`,
        );
      }
      if (detectedFamily !== null && !slugs.has(detectedFamily)) {
        throw new TypeError(
          `the gateway returned unknown detected chat-template family ${detectedFamily}`,
        );
      }
      return {
        name: requireString(model["name"], "chat-template model name"),
        effective_source: source,
        effective_family: effectiveFamily,
        detected_family: detectedFamily,
        reason: requireString(model["reason"], "chat-template resolution reason"),
      };
    });
    return { families, mappings, models };
  }

  /** Opens the OS file manager at `path` via `POST /admin/reveal`. */
  async reveal(path: string): Promise<void> {
    const response = await this.send("/admin/reveal", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ path }),
    });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /**
   * Fetches the cached cloud provider model sheet via
   * `GET /admin/cloud-models`. While no sheet has arrived the gateway
   * answers 503 and this throws a {@link GatewayHttpError} whose code is
   * `cloud_models_loading`; a failed download throws with
   * `cloud_models_unavailable`. Callers branch on the code to poll.
   */
  async getCloudModels(signal?: AbortSignal): Promise<CloudSheet> {
    const response = await this.send("/admin/cloud-models", { signal });
    if (!response.ok) {
      throw await refusalError(response);
    }
    return parseCloudSheet(await response.json());
  }

  /**
   * Forces a re-download of the cloud model sheet via
   * `POST /admin/cloud-models/refresh`; the gateway awaits the download
   * and answers 200 with the fresh sheet, or a refusal envelope when the
   * download fails.
   */
  async refreshCloudModels(): Promise<CloudSheet> {
    const response = await this.send("/admin/cloud-models/refresh", { method: "POST" });
    if (!response.ok) {
      throw await refusalError(response);
    }
    return parseCloudSheet(await response.json());
  }

  /** Lists the cached blobs via `GET /v1/cache`. */
  async listCache(): Promise<CacheListEntry[]> {
    const data = await this.getJson("/v1/cache");
    if (!Array.isArray(data)) {
      return [];
    }
    return data.flatMap((item) => {
      if (
        !isRecord(item) ||
        typeof item["source"] !== "string" ||
        typeof item["path"] !== "string" ||
        typeof item["sha256"] !== "string" ||
        !/^[0-9a-f]{64}$/i.test(item["sha256"]) ||
        typeof item["size_bytes"] !== "number"
      ) {
        return [];
      }
      return [
        {
          source: item["source"],
          path: item["path"],
          sha256: item["sha256"].toLowerCase(),
          size_bytes: item["size_bytes"],
        },
      ];
    });
  }

  /** Deletes a cached artifact via `DELETE /v1/cache/{sha256}`. */
  async deleteCached(sha256: string): Promise<void> {
    const response = await this.send(`/v1/cache/${sha256}`, { method: "DELETE" });
    if (!response.ok) {
      throw await refusalError(response);
    }
  }

  /**
   * Subscribes to the `GET /admin/progress` SSE stream, invoking
   * `onEvent` with each parsed {@link Progress} snapshot: the current one
   * first, then one per change. Returns the unsubscribe function. A
   * transport failure ends the subscription quietly; a 401 flows through
   * the shared unauthorized path.
   */
  subscribeProgress(onEvent: (event: Progress) => void): () => void {
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
            onEvent(parseProgress(JSON.parse(payload)));
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

  /** Fetches the host-metrics snapshot. */
  async getSystem(signal?: AbortSignal): Promise<SystemSnapshot> {
    const data = requireRecord(await this.getJson("/admin/system", signal), "system snapshot");
    const cpu = optionalRecord(data["cpu"]);
    const ram = optionalRecord(data["ram"]);
    const disk = optionalRecord(data["disk"]);
    const gpu = optionalRecord(data["gpu"]);
    return {
      cpu: cpu
        ? {
            frequency_mhz: numberOrZero(cpu["frequency_mhz"]),
            logical_cores: numberOrZero(cpu["logical_cores"]),
            physical_cores:
              typeof cpu["physical_cores"] === "number" ? cpu["physical_cores"] : null,
            utilization_percent: numberOrZero(cpu["utilization_percent"]),
          }
        : null,
      ram: {
        used_bytes: numberOrZero(ram?.["used_bytes"]),
        total_bytes: numberOrZero(ram?.["total_bytes"]),
      },
      disk: disk
        ? {
            cache_dir: typeof disk["cache_dir"] === "string" ? disk["cache_dir"] : "",
            used_bytes: numberOrZero(disk["used_bytes"]),
            total_bytes: numberOrZero(disk["total_bytes"]),
          }
        : null,
      gpu: gpu
        ? {
            name: typeof gpu["name"] === "string" ? gpu["name"] : "",
            vram_used_bytes: numberOrZero(gpu["vram_used_bytes"]),
            vram_total_bytes: numberOrZero(gpu["vram_total_bytes"]),
          }
        : null,
    };
  }

  /**
   * Proxied hub search via `GET /admin/hf/search`. `params` are the
   * hub's own query parameters (`q`, `sort`, `direction`, `filter`,
   * `limit`); the JSON body comes back verbatim.
   */
  async hfSearch(
    params: ReadonlyArray<readonly [string, string]>,
    signal?: AbortSignal,
  ): Promise<unknown> {
    const search = new URLSearchParams();
    for (const [key, value] of params) {
      search.append(key, value);
    }
    const query = search.toString();
    const response = await this.sendHf(`/admin/hf/search?${query}`, signal);
    return response.json();
  }

  /**
   * Proxied hub README via `GET /admin/hf/model/{owner}/{name}/readme`.
   * Returns the markdown text on 200, `null` on 404 (no README).
   */
  async hfReadme(repo: string, signal?: AbortSignal): Promise<string | null> {
    const encoded = repo.split("/").map(encodeURIComponent).join("/");
    const key = this.storage.getItem(API_KEY_STORAGE_KEY) ?? "";
    const response = await this.transport(
      this.base + `/admin/hf/model/${encoded}/readme`,
      { headers: { Authorization: `Bearer ${key}` }, signal },
    );
    if (response.status === 404) {
      return null;
    }
    if (response.status === 401) {
      let code = "";
      try {
        const body = optionalRecord(await response.json());
        const error = optionalRecord(body?.["error"]);
        code = typeof error?.["code"] === "string" ? error["code"] : "";
      } catch {
        // A bodyless 401 is treated as the gateway's own refusal.
      }
      if (code === "upstream_client_error") {
        throw new HfAuthError();
      }
      this.clearKey();
      this.onUnauthorized?.();
      throw new UnauthorizedError();
    }
    if (!response.ok) {
      throw await refusalError(response);
    }
    return response.text();
  }

  /**
   * Proxied hub model detail via `GET /admin/hf/model/{owner}/{name}`;
   * the gateway adds `blobs=true`, so siblings carry exact file sizes.
   */
  async hfModel(repo: string, signal?: AbortSignal): Promise<unknown> {
    const encoded = repo.split("/").map(encodeURIComponent).join("/");
    const response = await this.sendHf(`/admin/hf/model/${encoded}`, signal);
    return response.json();
  }

  /**
   * Sends one HF-proxy request. A 401 here is ambiguous: the hub's own
   * refusal (no HF_TOKEN) passes through the proxy with the
   * `upstream_client_error` code and becomes {@link HfAuthError}, while
   * the gateway refusing the SPA's bearer key follows the shared
   * unauthorized path.
   */
  private async sendHf(path: string, signal?: AbortSignal): Promise<Response> {
    const key = this.storage.getItem(API_KEY_STORAGE_KEY) ?? "";
    const response = await this.transport(this.base + path, {
      headers: { Authorization: `Bearer ${key}` },
      signal,
    });
    if (response.status === 401) {
      let code = "";
      try {
        const body = optionalRecord(await response.json());
        const error = optionalRecord(body?.["error"]);
        code = typeof error?.["code"] === "string" ? error["code"] : "";
      } catch {
        // A bodyless 401 is treated as the gateway's own refusal.
      }
      if (code === "upstream_client_error") {
        throw new HfAuthError();
      }
      this.clearKey();
      this.onUnauthorized?.();
      throw new UnauthorizedError();
    }
    if (!response.ok) {
      throw await refusalError(response);
    }
    return response;
  }

  /** GETs a JSON endpoint, throwing on any non-success status. */
  private async getJson(path: string, signal?: AbortSignal): Promise<unknown> {
    const response = await this.send(path, { signal });
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
    const headers = new Headers(init.headers);
    headers.set("Authorization", `Bearer ${key}`);
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
      if (!init.signal?.aborted && !isAbortError(error)) {
        this.onHealth?.(false);
      }
      throw error;
    }
    this.onHealth?.(true);
    return response;
  }
}

/** Whether an async failure is the expected result of caller cancellation. */
function isAbortError(error: unknown): boolean {
  return (
    error !== null &&
    typeof error === "object" &&
    "name" in error &&
    error.name === "AbortError"
  );
}

/**
 * Builds the {@link GatewayHttpError} for a refused request, carrying
 * the envelope's `error.code` when the gateway sent one.
 */
async function refusalError(response: Response): Promise<GatewayHttpError> {
  const refusal = await refusalDetail(response);
  return new GatewayHttpError(response.status, refusal.message, refusal.code);
}

/**
 * Extracts the message and, when the envelope is the gateway's
 * `{"error": {"message", "code", ...}}` shape, its `code` from a
 * buffered refusal response.
 */
async function refusalDetail(
  response: Response,
): Promise<{ message: string; code: string | null }> {
  try {
    const body = requireRecord(await response.json(), "error response");
    const error = body["error"];
    if (typeof error === "string") {
      return { message: error, code: null };
    }
    if (isRecord(error)) {
      const message = error["message"];
      if (typeof message === "string") {
        const code = error["code"];
        return { message, code: typeof code === "string" ? code : null };
      }
    }
  } catch {
    // Fall through to the status line.
  }
  return { message: `the gateway refused the switch (${response.status})`, code: null };
}

/** Requires an object at an external JSON boundary. */
function requireRecord(value: unknown, label: string): Record<string, unknown> {
  if (!isRecord(value)) {
    throw new TypeError(`the gateway returned invalid ${label} JSON`);
  }
  return value;
}

/** Requires an external JSON array. */
function requireArray(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new TypeError(`the gateway returned invalid ${label} JSON`);
  }
  return value;
}

/** Requires an external JSON string. */
function requireString(value: unknown, label: string): string {
  if (typeof value !== "string") {
    throw new TypeError(`the gateway returned invalid ${label} JSON`);
  }
  return value;
}

/** Requires a string-or-null external field. */
function nullableString(value: unknown, label: string): string | null {
  if (value === null) {
    return null;
  }
  return requireString(value, label);
}

/** Requires a launch-template source category. */
function requireChatTemplateSource(value: unknown): ChatTemplateSource {
  if (
    value === "embedded" ||
    value === "known-override" ||
    value === "builtin" ||
    value === "custom"
  ) {
    return value;
  }
  throw new TypeError("the gateway returned invalid effective chat-template source JSON");
}

/** Returns an object or null for an optional external object. */
function optionalRecord(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

/** Keeps only string members from an external array. */
function stringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((entry): entry is string => typeof entry === "string")
    : [];
}

/** Keeps only string values from an external object. */
function stringRecord(value: unknown): Record<string, string> {
  if (!isRecord(value)) {
    return {};
  }
  return Object.fromEntries(
    Object.entries(value).filter(
      (entry): entry is [string, string] => typeof entry[1] === "string",
    ),
  );
}

/**
 * Reads a `Progress` snapshot off an external value; anything malformed
 * reads as idle, so a stale bar clears rather than sticks.
 */
function parseProgress(value: unknown): Progress {
  const record = optionalRecord(value);
  return {
    busy: record?.["busy"] === true,
    text: typeof record?.["text"] === "string" ? record["text"] : "",
  };
}

/** Reads a finite external number, defaulting malformed values to zero. */
function numberOrZero(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/** Reads an optional finite external number. */
function numberOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Parses one sheet env var, dropping malformed entries. */
function parseCloudEnvVar(raw: unknown): CloudEnvVar[] {
  if (!isRecord(raw) || typeof raw["name"] !== "string") {
    return [];
  }
  return [
    {
      name: raw["name"],
      role: raw["role"] === "key" ? "key" : "config",
      default: typeof raw["default"] === "string" ? raw["default"] : null,
    },
  ];
}

/** Parses one sheet model entry, dropping malformed entries. */
function parseCloudModel(raw: unknown): CloudModelEntry[] {
  if (!isRecord(raw) || typeof raw["id"] !== "string") {
    return [];
  }
  const thinking = isRecord(raw["thinking"]) ? raw["thinking"] : {};
  const pricing = isRecord(raw["pricing"]) ? raw["pricing"] : null;
  return [
    {
      id: raw["id"],
      display_name: typeof raw["display_name"] === "string" ? raw["display_name"] : raw["id"],
      family: typeof raw["family"] === "string" ? raw["family"] : "",
      variant_of: typeof raw["variant_of"] === "string" ? raw["variant_of"] : null,
      variant: typeof raw["variant"] === "string" ? raw["variant"] : null,
      languages: stringArray(raw["languages"]),
      kind: typeof raw["kind"] === "string" ? raw["kind"] : "chat",
      context_window: numberOrNull(raw["context_window"]),
      max_output: numberOrNull(raw["max_output"]),
      images: raw["images"] === true,
      tool_calling: raw["tool_calling"] === true,
      thinking: {
        supported: thinking["supported"] === true,
        enabled: thinking["enabled"] === true,
        adaptive: thinking["adaptive"] === true,
      },
      effort_levels: stringArray(raw["effort_levels"]),
      default_effort: typeof raw["default_effort"] === "string" ? raw["default_effort"] : null,
      pricing: pricing
        ? {
            currency: typeof pricing["currency"] === "string" ? pricing["currency"] : "USD",
            prompt_per_mtok: numberOrZero(pricing["prompt_per_mtok"]),
            completion_per_mtok: numberOrZero(pricing["completion_per_mtok"]),
          }
        : null,
    },
  ];
}

/** Parses the `GET /admin/cloud-models` envelope tolerantly. */
function parseCloudSheet(raw: unknown): CloudSheet {
  const data = requireRecord(raw, "cloud model sheet");
  const providers: Record<string, CloudProviderSlice> = {};
  if (isRecord(data["providers"])) {
    for (const [name, value] of Object.entries(data["providers"])) {
      if (!isRecord(value)) {
        continue;
      }
      providers[name] = {
        display_name: typeof value["display_name"] === "string" ? value["display_name"] : name,
        tier: typeof value["tier"] === "string" ? value["tier"] : "niche",
        status: typeof value["status"] === "string" ? value["status"] : "ok",
        openai_base_url:
          typeof value["openai_base_url"] === "string" ? value["openai_base_url"] : null,
        env_vars: Array.isArray(value["env_vars"])
          ? value["env_vars"].flatMap(parseCloudEnvVar)
          : [],
        models: Array.isArray(value["models"])
          ? value["models"].flatMap(parseCloudModel)
          : [],
      };
    }
  }
  return {
    schema_version: numberOrZero(data["schema_version"]),
    generated_at: typeof data["generated_at"] === "string" ? data["generated_at"] : "",
    providers,
  };
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
