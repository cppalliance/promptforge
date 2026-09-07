// The config store: running config, pending (shadow-overlaid) config,
// the dirty report, and the browser-side edit state. The three states
// the write path defines: *dirty* is an unsaved edit held here, keyed
// per model and field; *pending* is a saved shadow, visible as the
// difference between the pending and running views; *applied* is the
// running config itself. Save builds the full PUT /admin/config payload
// from the pending view plus one model's edits - untouched secrets ride
// through as the "***" the gateway sent, so no real secret ever leaves
// or re-enters the browser.
import { GatewayHttpError } from "./gateway-api";
import type {
  CacheListEntry,
  ChatTemplateCatalog,
  ChatTemplateFamily,
  ChatTemplateModelResolution,
  DirtyReport,
  GatewayApi,
  OrphanFile,
} from "./gateway-api";
import { isRecord } from "./json";

/** Which TOML catalog array a model entry lives in. */
export type ModelSource = "local" | "remote" | "stt";

/** A JSON object: one config entry's fields. */
export type EntryData = Record<string, unknown>;

/** One model from the global catalog, with pending state. */
export interface ModelEntry {
  /** The catalog array kind. */
  kind: ModelSource;
  /** The entry's `name` key (the catalog identity). */
  name: string;
  /** The pending view's values for this entry, provenance stripped. */
  data: EntryData;
  /** Field keys whose pending value differs from the running value. */
  pendingFields: ReadonlySet<string>;
  /** True for a browser-created entry that has never been saved. */
  draft: boolean;
}

/** The keyed arrays and the identity field each one merges by. */
const KEYED_ARRAYS: ReadonlyArray<readonly [array: string, key: string]> = [
  ["dominion", "id"],
  ["endpoint", "id"],
  ["model", "name"],
  ["local_model", "name"],
  ["stt_model", "name"],
  ["profile", "name"],
];

/** One `[[dominion]]` or `[[endpoint]]` entry with pending state. */
export interface SectionEntry {
  /** The entry's `id` key. */
  id: string;
  /** The pending view's values, provenance stripped. */
  data: EntryData;
  /** Field keys whose pending value differs from the running value. */
  pendingFields: ReadonlySet<string>;
}

/** One named profile checklist from the pending document. */
export interface ProfileEntry {
  /** Profile identifier. */
  name: string;
  /** Chosen catalog names in their stored order. */
  models: string[];
}

/** Reads a dot-path (`speculative.draft_max`) out of a JSON object. */
export function readPath(data: EntryData, key: string): unknown {
  let value: unknown = data;
  for (const part of key.split(".")) {
    if (value === null || typeof value !== "object") {
      return undefined;
    }
    value = (value as EntryData)[part];
  }
  return value;
}

/** Writes a dot-path into a JSON object, creating intermediate objects. */
export function writePath(data: EntryData, key: string, value: unknown): void {
  const parts = key.split(".");
  let target = data;
  for (const part of parts.slice(0, -1)) {
    const next = target[part];
    if (next === null || typeof next !== "object" || Array.isArray(next)) {
      target[part] = {};
    }
    target = target[part] as EntryData;
  }
  const leaf = parts[parts.length - 1] ?? key;
  target[leaf] = value;
}

/** Structural equality through JSON, sufficient for config values. */
function sameValue(a: unknown, b: unknown): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

/** One changed path in the pending-vs-running Review diff. */
export interface DiffRow {
  /** The dotted path, keyed-array entries by identity (`endpoint[openai].base_url`). */
  path: string;
  /** The running (applied) value; undefined when the path is new. */
  running: unknown;
  /** The pending (shadow) value; undefined when the path was removed. */
  pending: unknown;
}

/**
 * The store. Constructed once per shell mount; views subscribe and read,
 * the composition root drives load/apply/revert.
 */
export class ConfigStore {
  /** Whether the initial load has finished (successfully or not). */
  loaded = false;
  /** The load failure message, when the initial load failed. */
  loadError: string | null = null;
  /** The dirty report from the latest refresh. */
  dirty: DirtyReport = { dirty: false, pending_files: [], changed_sections: [] };
  /** Unconfigured cache files from the latest refresh. */
  orphans: OrphanFile[] = [];
  /** Cache metadata used for per-model file status. */
  cache: CacheListEntry[] = [];
  /** The active profile's name, from `GET /admin/status`. */
  activeProfile = "";
  /**
   * Counts completed reverts. A view that holds unsaved edits of its own
   * compares this on notify and discards them when it moved, so Revert
   * All returns every pane to the running configuration.
   */
  revertGeneration = 0;
  /** Server-owned template families, mapper, and effective decisions. */
  private chatTemplates: ChatTemplateCatalog = { families: [], mappings: [], models: [] };

  private readonly api: GatewayApi;
  private running: EntryData = {};
  private pending: EntryData = {};
  /** Names of models the running profile exposes (the status list). */
  private runningModels: string[] = [];
  /** Unsaved edits: entry key -> field key -> value. */
  private readonly edits = new Map<string, Map<string, unknown>>();
  /** Browser-created entries not yet saved. */
  private drafts: { kind: ModelSource; data: EntryData }[] = [];
  /** Serializes full-config writes started by Discover. */
  private stageTail: Promise<void> = Promise.resolve();
  private readonly listeners = new Set<() => void>();

  constructor(api: GatewayApi) {
    this.api = api;
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

  /** Loads everything the models view needs; failures land in `loadError`. */
  async load(): Promise<void> {
    try {
      const [running, pending, dirty, orphans, cache, status, chatTemplates] = await Promise.all([
        this.api.getConfig(),
        this.api.getConfigPending(),
        this.api.getConfigDirty(),
        // Headless builds lack /admin/orphans (local feature); the list
        // degrades to empty instead of failing the whole load.
        this.api.getOrphans().catch((): OrphanFile[] => []),
        this.api.listCache().catch((): CacheListEntry[] => []),
        this.api.getStatus(),
        this.loadChatTemplates(),
      ]);
      this.running = running;
      this.pending = pending;
      this.dirty = dirty;
      this.orphans = visibleOrphans(orphans);
      this.cache = cache;
      this.activeProfile = status.profile;
      this.runningModels = status.models;
      this.chatTemplates = chatTemplates;
      this.loadError = null;
    } catch (error) {
      this.loadError = error instanceof Error ? error.message : String(error);
    }
    this.loaded = true;
    this.notify();
  }

  /** Re-reads the pending view and the dirty report after a write. */
  private async refreshPending(): Promise<void> {
    const [pending, dirty, chatTemplates] = await Promise.all([
      this.api.getConfigPending(),
      this.api.getConfigDirty(),
      this.loadChatTemplates(),
    ]);
    this.pending = pending;
    this.dirty = dirty;
    this.chatTemplates = chatTemplates;
  }

  /** Reads the local-only catalog while preserving remote-only headless builds. */
  private async loadChatTemplates(): Promise<ChatTemplateCatalog> {
    try {
      return await this.api.getChatTemplates();
    } catch (error) {
      if (error instanceof GatewayHttpError && error.status === 404) {
        return { families: [], mappings: [], models: [] };
      }
      throw error;
    }
  }

  /** Re-reads the running view too (after apply/revert). */
  private async refreshAll(): Promise<void> {
    const [running, status] = await Promise.all([this.api.getConfig(), this.api.getStatus()]);
    this.running = running;
    this.activeProfile = status.profile;
    this.runningModels = status.models;
    await Promise.all([this.refreshPending(), this.refreshArtifacts()]);
  }

  /** Re-reads the cache and orphan lists after adoption or deletion. */
  async refreshOrphans(): Promise<void> {
    await this.refreshArtifacts();
    this.notify();
  }

  /** Re-reads cache metadata and hides ArtifactStore bookkeeping. */
  private async refreshArtifacts(): Promise<void> {
    const [orphans, cache] = await Promise.all([
      this.api.getOrphans().catch((): OrphanFile[] => []),
      this.api.listCache().catch((): CacheListEntry[] => []),
    ]);
    this.orphans = visibleOrphans(orphans);
    this.cache = cache;
  }

  /** The merged model catalog: pending entries plus unsaved drafts. */
  models(): ModelEntry[] {
    const entries: ModelEntry[] = [];
    for (const [array, kind] of [
      ["model", "remote"],
      ["local_model", "local"],
      ["stt_model", "stt"],
    ] as const) {
      const pendingEntries = this.entriesOf(this.pending, array);
      const runningByName = new Map(
        this.entriesOf(this.running, array).map((entry) => [String(entry["name"] ?? ""), entry]),
      );
      for (const raw of pendingEntries) {
        const data = { ...raw };
        const name = String(data["name"] ?? "");
        entries.push({
          kind,
          name,
          data,
          pendingFields: this.diffFields(data, runningByName.get(name)),
          draft: false,
        });
      }
    }
    for (const draft of this.drafts) {
      entries.push({
        kind: draft.kind,
        name: String(draft.data["name"] ?? ""),
        data: draft.data,
        pendingFields: new Set(),
        draft: true,
      });
    }
    return entries;
  }

  /** Bundled chat-template families in server catalog order. */
  chatTemplateFamilies(): readonly ChatTemplateFamily[] {
    return this.chatTemplates.families;
  }

  /** Effective server-side template resolution for one configured model. */
  chatTemplateResolution(name: string): ChatTemplateModelResolution | null {
    return this.chatTemplates.models.find((model) => model.name === name) ?? null;
  }

  /** Exact server-side family mapping for a Hugging Face repository ID. */
  mappedChatTemplateFamily(modelId: string): string | null {
    const normalized = modelId.trim().toLowerCase();
    return (
      this.chatTemplates.mappings.find((mapping) => mapping.model_id === normalized)?.family ?? null
    );
  }

  /** Finds one entry by kind and name (drafts included). */
  findModel(kind: ModelSource, name: string): ModelEntry | null {
    return this.models().find((entry) => entry.kind === kind && entry.name === name) ?? null;
  }

  /** Finds one entry by name alone for Local and Remote detail routes. */
  findByName(name: string): ModelEntry | null {
    return this.models().find((entry) => entry.name === name) ?? null;
  }

  /** Cache metadata for a local or STT model's source, when downloaded. */
  cachedFile(entry: ModelEntry): CacheListEntry | null {
    if (entry.kind === "remote") {
      return null;
    }
    const source = entry.data["source"];
    if (typeof source !== "string" || source === "") {
      return null;
    }
    const normalized = source.replaceAll("\\", "/").toLocaleLowerCase();
    const relative = normalized.replace(/^\.?\//, "");
    return (
      this.cache.find(
        (cached) => {
          const cachedPath = cached.path.replaceAll("\\", "/").toLocaleLowerCase();
          return (
            cached.source === source ||
            cachedPath === normalized ||
            cachedPath.endsWith(`/${relative}`)
          );
        },
      ) ?? null
    );
  }

  /** Every pending profile checklist in declaration order. */
  profiles(): ProfileEntry[] {
    return this.entriesOf(this.pending, "profile").map((entry) => ({
      name: String(entry["name"] ?? ""),
      models: Array.isArray(entry["models"]) ? entry["models"].map(String) : [],
    }));
  }

  /** The active profile staged for Apply, falling back to the running one. */
  pendingActiveProfile(): string {
    const pending = this.pending["active_profile"];
    return typeof pending === "string" ? pending : this.activeProfile;
  }

  /** Profiles whose checklists contain `modelName`. */
  affectedProfiles(modelName: string): string[] {
    return this.profiles()
      .filter((profile) => profile.models.includes(modelName))
      .map((profile) => profile.name);
  }

  /** The `[[dominion]]` entries of the pending view. */
  dominions(): { id: string; kind: string; vramGb: number | null }[] {
    return this.entriesOf(this.pending, "dominion").map((entry) => ({
      id: String(entry["id"] ?? ""),
      kind: String(entry["kind"] ?? "remote"),
      vramGb: typeof entry["vram_gb"] === "number" ? entry["vram_gb"] : null,
    }));
  }

  /** A dot-path read of the pending view (`server.bind`, `tools.web_search`). */
  sectionValue(path: string): unknown {
    return readPath(this.pending, path);
  }

  /** A dot-path read of the running (applied) view. */
  runningSectionValue(path: string): unknown {
    return readPath(this.running, path);
  }

  /** The id-keyed entries of one Settings array with pending state. */
  keyedEntries(array: "dominion" | "endpoint"): SectionEntry[] {
    const runningById = new Map(
      this.entriesOf(this.running, array).map((entry) => [String(entry["id"] ?? ""), entry]),
    );
    return this.entriesOf(this.pending, array).map((raw) => {
      const data = { ...raw };
      const id = String(data["id"] ?? "");
      return {
        id,
        data,
        pendingFields: this.diffFields(data, runningById.get(id)),
      };
    });
  }

  /** The model catalog entries of the pending view (raw). */
  modelEntriesRaw(): { array: "model" | "local_model" | "stt_model"; data: EntryData }[] {
    const rows: { array: "model" | "local_model" | "stt_model"; data: EntryData }[] = [];
    for (const array of ["model", "local_model", "stt_model"] as const) {
      for (const data of this.entriesOf(this.pending, array)) {
        rows.push({ array, data });
      }
    }
    return rows;
  }

  /**
   * The full `PUT /admin/config` payload base. Untouched secrets remain
   * `"***"` so the gateway can restore them before validation.
   */
  buildConfigPayload(): EntryData {
    return structuredClone(this.pending);
  }

  /** Stages the global config and optional active-profile shadow. */
  async savePayload(payload: EntryData): Promise<void> {
    await this.api.putConfig(payload);
    await this.refreshPending();
    this.notify();
  }

  /** Saves one profile checklist in catalog order. */
  async saveProfile(name: string, chosen: readonly string[]): Promise<void> {
    const payload = this.buildConfigPayload();
    const catalogOrder = new Map(this.models().map((entry, index) => [entry.name, index]));
    const ordered = [...new Set(chosen)].sort(
      (left, right) =>
        (catalogOrder.get(left) ?? Number.MAX_SAFE_INTEGER) -
        (catalogOrder.get(right) ?? Number.MAX_SAFE_INTEGER),
    );
    const profile = this.entriesOf(payload, "profile").find((entry) => entry["name"] === name);
    if (!profile) {
      throw new Error(`profile ${name} is not available`);
    }
    profile["models"] = ordered;
    await this.savePayload(payload);
  }

  /** Creates an empty profile or a copy of another pending checklist. */
  async createProfile(name: string, copyFrom: string | null): Promise<void> {
    const payload = this.buildConfigPayload();
    const profiles = this.entriesOf(payload, "profile");
    const source = copyFrom === null ? null : profiles.find((entry) => entry["name"] === copyFrom);
    const models =
      source && Array.isArray(source["models"]) ? source["models"].map(String) : [];
    profiles.push({ name, models });
    payload["profile"] = profiles;
    await this.savePayload(payload);
  }

  /** Deletes a non-active profile from the pending document. */
  async deleteProfile(name: string): Promise<void> {
    const payload = this.buildConfigPayload();
    payload["profile"] = this.entriesOf(payload, "profile").filter(
      (entry) => entry["name"] !== name,
    );
    await this.savePayload(payload);
  }

  /** Stages the active-profile pointer without switching the live runtime. */
  async stageActiveProfile(name: string): Promise<void> {
    const payload = this.buildConfigPayload();
    payload["active_profile"] = name;
    await this.savePayload(payload);
  }

  /** Creates or resets the digest-pinned recommended STT catalog pair. */
  async restoreSttModels(recommended: readonly EntryData[]): Promise<void> {
    const payload = this.buildConfigPayload();
    const current = this.entriesOf(payload, "stt_model");
    const replacements = new Map(
      recommended.map((entry) => [String(entry["name"] ?? ""), structuredClone(entry)]),
    );
    const merged = current.map((entry) => {
      const replacement = replacements.get(String(entry["name"] ?? ""));
      if (!replacement) {
        return entry;
      }
      replacements.delete(String(entry["name"] ?? ""));
      return {
        ...entry,
        source: replacement["source"],
        sha256: replacement["sha256"],
        vram_gb: replacement["vram_gb"],
      };
    });
    merged.push(...replacements.values());
    payload["stt_model"] = merged;
    await this.savePayload(payload);
  }

  /** The `[[endpoint]]` ids of the pending view. */
  endpointIds(): string[] {
    return this.entriesOf(this.pending, "endpoint").map((entry) => String(entry["id"] ?? ""));
  }

  /** Whether the running profile exposes (runs) the named model. */
  isRunning(name: string): boolean {
    return this.runningModels.includes(name);
  }

  /** The RUNNING (applied) value of a field, or undefined when the entry is new. */
  runningValue(entry: ModelEntry, key: string): unknown {
    const array = modelArray(entry.kind);
    const running = this.entriesOf(this.running, array).find(
      (item) => item["name"] === entry.name,
    );
    return running ? readPath(running, key) : undefined;
  }

  /** The current value of a field: the unsaved edit, else the loaded value. */
  value(entry: ModelEntry, key: string): unknown {
    const edits = this.edits.get(entryKey(entry));
    if (edits?.has(key)) {
      return edits.get(key);
    }
    return readPath(entry.data, key);
  }

  /** Whether the field carries an unsaved edit. */
  isEdited(entry: ModelEntry, key: string): boolean {
    return this.edits.get(entryKey(entry))?.has(key) ?? false;
  }

  /** Whether the entry carries any unsaved edit (drafts always do). */
  hasEdits(entry: ModelEntry): boolean {
    return entry.draft || (this.edits.get(entryKey(entry))?.size ?? 0) > 0;
  }

  /**
   * Records an edit. An edit equal to the loaded value clears itself,
   * so resetting by retyping the original works like the reset button.
   */
  setEdit(entry: ModelEntry, key: string, value: unknown): void {
    if (entry.draft) {
      // A draft has no saved baseline; write straight into its data.
      writePath(entry.data, key, value);
      this.notify();
      return;
    }
    const id = entryKey(entry);
    let edits = this.edits.get(id);
    if (!edits) {
      edits = new Map();
      this.edits.set(id, edits);
    }
    if (sameValue(value, readPath(entry.data, key))) {
      edits.delete(key);
    } else {
      edits.set(key, value);
    }
    this.notify();
  }

  /** Reverts one field to its loaded value. */
  resetEdit(entry: ModelEntry, key: string): void {
    this.edits.get(entryKey(entry))?.delete(key);
    this.notify();
  }

  /** Reverts every unsaved edit on the entry. */
  resetEntry(entry: ModelEntry): void {
    this.edits.delete(entryKey(entry));
    this.notify();
  }

  /** Creates an unsaved draft entry and returns its unique name. */
  addDraft(kind: ModelSource, data: EntryData): string {
    let name = String(data["name"] ?? "new-model");
    const taken = new Set(this.models().map((entry) => entry.name));
    let candidate = name;
    let counter = 2;
    while (taken.has(candidate)) {
      candidate = `${name}-${counter}`;
      counter += 1;
    }
    name = candidate;
    this.drafts.push({ kind, data: { ...data, name } });
    this.notify();
    return name;
  }

  /**
   * Stages a discovered local or STT model and chooses it in the pending
   * active profile, so Apply provisions the artifact.
   */
  async stageDiscoveredModel(kind: "local" | "stt", data: EntryData): Promise<string> {
    const operation = this.stageTail.then(async (): Promise<string> => {
      const payload = this.buildConfigPayload();
      const array = modelArray(kind);
      const items = this.entriesOf(payload, array);
      const taken = new Set(this.models().map((entry) => entry.name));
      const base = String(data["name"] ?? (kind === "stt" ? "new-stt-model" : "new-local-model"));
      let name = base;
      let counter = 2;
      while (taken.has(name)) {
        name = `${base}-${counter}`;
        counter += 1;
      }
      items.push({ ...structuredClone(data), name });
      payload[array] = items;
      const active = this.pendingActiveProfile();
      for (const profile of this.entriesOf(payload, "profile")) {
        if (profile["name"] !== active) {
          continue;
        }
        const chosen = Array.isArray(profile["models"]) ? profile["models"].map(String) : [];
        profile["models"] = [...chosen, name];
      }
      await this.api.putConfig(payload);
      await this.refreshPending();
      this.notify();
      return name;
    });
    this.stageTail = operation.then(
      () => undefined,
      () => undefined,
    );
    return operation;
  }

  /** Discards a draft without saving it. */
  discardDraft(entry: ModelEntry): void {
    this.drafts = this.drafts.filter((draft) => draft.data !== entry.data);
    this.notify();
  }

  /**
   * The full `PUT /admin/config` payload: the pending view stripped of
   * provenance, with `entry`'s unsaved edits applied (or the entry
   * removed when `remove` is set). Secrets the user never touched are
   * still the `"***"` the gateway sent, so the gateway preserves them.
   */
  buildSavePayload(entry: ModelEntry, remove = false): EntryData {
    const payload = this.buildConfigPayload();
    const array = modelArray(entry.kind);
    let items = this.entriesOf(payload, array);
    if (entry.draft) {
      items.push(structuredClone(entry.data));
      payload[array] = items;
      return payload;
    }
    if (remove) {
      payload[array] = items.filter((item) => item["name"] !== entry.name);
      for (const profile of this.entriesOf(payload, "profile")) {
        const names = Array.isArray(profile["models"]) ? profile["models"].map(String) : [];
        profile["models"] = names.filter((name) => name !== entry.name);
      }
      return payload;
    }
    const target = items.find((item) => item["name"] === entry.name);
    if (target) {
      const edits = this.edits.get(entryKey(entry));
      for (const [key, value] of edits ?? []) {
        writePath(target, key, value);
      }
      const renamed = String(target["name"] ?? entry.name);
      if (renamed !== entry.name) {
        for (const profile of this.entriesOf(payload, "profile")) {
          const names = Array.isArray(profile["models"]) ? profile["models"].map(String) : [];
          profile["models"] = names.map((name) => (name === entry.name ? renamed : name));
        }
      }
    }
    return payload;
  }

  /** Saves one entry's edits (or the draft itself) to the shadow. */
  async save(entry: ModelEntry): Promise<void> {
    await this.api.putConfig(this.buildSavePayload(entry));
    if (entry.draft) {
      this.discardDraft(entry);
    } else {
      this.edits.delete(entryKey(entry));
    }
    await this.refreshPending();
    this.notify();
  }

  /** Removes one entry from the config and saves the shadow. */
  async deleteModel(entry: ModelEntry): Promise<void> {
    if (entry.draft) {
      this.discardDraft(entry);
      return;
    }
    await this.api.putConfig(this.buildSavePayload(entry, true));
    this.edits.delete(entryKey(entry));
    await this.refreshPending();
    this.notify();
  }

  /** Promotes every shadow and refreshes; returns the apply outcome. */
  async apply(): Promise<{ restart_required: boolean }> {
    const outcome = await this.api.applyConfig();
    await this.refreshAll();
    this.notify();
    return outcome;
  }

  /**
   * Deletes every shadow, discards unsaved edits and drafts, and
   * refreshes. Revert All promises the running configuration, so edits
   * layered on the shadows go with them: otherwise their dirty dots and
   * Save/Reset controls outlive the pending state they sat on.
   */
  async revertAll(): Promise<void> {
    await this.api.revertConfig();
    this.edits.clear();
    this.drafts = [];
    this.revertGeneration += 1;
    await this.refreshAll();
    this.notify();
  }

  /**
   * The pending-vs-running diff [INVENTED] behind the banner's Review
   * action: every path whose pending value differs from the running one,
   * as rows of `path | running | pending`. Keyed-array entries match by
   * identity (`endpoint[openai].base_url`); other arrays diff wholesale
   * as one row. Both views arrive with secrets redacted to `"***"`, so
   * no row ever carries credential material.
   */
  pendingDiff(): DiffRow[] {
    const keyed = new Map<string, string>(KEYED_ARRAYS);
    const rows: DiffRow[] = [];
    const compare = (running: unknown, pending: unknown, path: string[]): void => {
      if (sameValue(running, pending)) {
        return;
      }
      const runningSide = isRecord(running) ? running : undefined;
      const pendingSide = isRecord(pending) ? pending : undefined;
      if (
        (runningSide || pendingSide) &&
        (running === undefined || runningSide) &&
        (pending === undefined || pendingSide)
      ) {
        const keys = new Set([
          ...Object.keys(runningSide ?? {}),
          ...Object.keys(pendingSide ?? {}),
        ]);
        for (const key of keys) {
          compare(runningSide?.[key], pendingSide?.[key], [...path, key]);
        }
        return;
      }
      const array = path[path.length - 1] ?? "";
      const idKey = keyed.get(array);
      if (idKey && Array.isArray(running ?? []) && Array.isArray(pending ?? [])) {
        const byId = (side: unknown): Map<string, EntryData> =>
          new Map(
            (Array.isArray(side) ? side : [])
              .filter(isRecord)
              .map((item) => [String(item[idKey] ?? ""), item]),
          );
        const runningById = byId(running);
        const pendingById = byId(pending);
        for (const id of new Set([...runningById.keys(), ...pendingById.keys()])) {
          compare(runningById.get(id), pendingById.get(id), [
            ...path.slice(0, -1),
            `${array}[${id}]`,
          ]);
        }
        return;
      }
      rows.push({ path: path.join("."), running, pending });
    };
    compare(this.running, this.pending, []);
    return rows;
  }

  /** One keyed array of a config JSON, as mutable objects. */
  private entriesOf(config: EntryData, array: string): EntryData[] {
    const value = config[array];
    if (!Array.isArray(value)) {
      return [];
    }
    return value.filter(
      (item): item is EntryData => item !== null && typeof item === "object",
    );
  }

  /** Field keys whose pending value differs from the running entry's. */
  private diffFields(pending: EntryData, running: EntryData | undefined): Set<string> {
    const diff = new Set<string>();
    if (!running) {
      // The whole entry is pending; every present field differs.
      for (const key of Object.keys(pending)) {
        diff.add(key);
      }
      return diff;
    }
    const keys = new Set([...Object.keys(pending), ...Object.keys(running)]);
    for (const key of keys) {
      if (!sameValue(pending[key], running[key])) {
        diff.add(key);
      }
    }
    return diff;
  }
}

/** The edit-map key for one entry. */
function entryKey(entry: ModelEntry): string {
  return `${entry.kind}:${entry.name}`;
}

/** The JSON array key for one catalog source. */
function modelArray(kind: ModelSource): "model" | "local_model" | "stt_model" {
  if (kind === "remote") {
    return "model";
  }
  return kind === "local" ? "local_model" : "stt_model";
}

/** Removes ArtifactStore marker files from a gateway response defensively. */
function visibleOrphans(orphans: OrphanFile[]): OrphanFile[] {
  return orphans.filter((orphan) => !orphan.path.toLocaleLowerCase().endsWith(".verified"));
}
