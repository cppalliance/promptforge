// The config store: running config, pending (shadow-overlaid) config,
// the dirty report, and the browser-side edit state. The three states
// the write path defines: *dirty* is an unsaved edit held here, keyed
// per model and field; *pending* is a saved shadow, visible as the
// difference between the pending and running views; *applied* is the
// running config itself. Save builds the full PUT /admin/config payload
// from the pending view plus one model's edits - untouched secrets ride
// through as the "***" the gateway sent, so no real secret ever leaves
// or re-enters the browser.

import type { CacheListEntry, DirtyReport, GatewayApi, OrphanFile } from "./gateway-api";

/** Which TOML array a model entry lives in. */
export type ModelSource = "local" | "remote";

/** A JSON object: one config entry's fields. */
export type EntryData = Record<string, unknown>;

/** One model from the merged catalog, with provenance and pending state. */
export interface ModelEntry {
  /** `local` for `[[local_model]]`, `remote` for `[[model]]`. */
  kind: ModelSource;
  /** The entry's `name` key (the catalog identity). */
  name: string;
  /** The pending view's values for this entry, provenance stripped. */
  data: EntryData;
  /** Base name of the file whose definition won the merge, when known. */
  sourceFile: string | null;
  /** Whether the winning definition came from an included parent file. */
  inherited: boolean;
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
];

/** One `[[dominion]]` or `[[endpoint]]` entry, with provenance and pending state. */
export interface SectionEntry {
  /** The entry's `id` key. */
  id: string;
  /** The pending view's values, provenance stripped. */
  data: EntryData;
  /** Base name of the file whose definition won the merge, when known. */
  sourceFile: string | null;
  /** Whether the winning definition came from an included parent file. */
  inherited: boolean;
  /** Field keys whose pending value differs from the running value. */
  pendingFields: ReadonlySet<string>;
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

/** The file base name of a provenance path (either separator). */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
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

/** One include-chain file from the pending view's `include` array. */
export interface ChainFile {
  /** The include entry verbatim as written in the leaf file (`common.toml`, `../gateway.toml`). */
  path: string;
  /** The file's base name (`common.toml`, `gateway.toml`). */
  base: string;
  /** Whether the entry points outside the profiles directory. */
  outside: boolean;
}

/**
 * The include chain read from a config payload's top-level `include`
 * array: the active profile leaf's own include line, verbatim and
 * ordered as written (the gateway serves the leaf shadow's array when
 * one is staged). Membership and order are authoritative, so a parent
 * whose every value a later file overrides still appears. An absent
 * array means the profile stands alone. The UI ships with its gateway,
 * so there is no fallback derivation for a payload without the field.
 */
export function includeChainOf(pending: EntryData): ChainFile[] {
  const include = pending["include"];
  if (!Array.isArray(include)) {
    return [];
  }
  return include
    .filter((entry): entry is string => typeof entry === "string")
    .map((entry) => ({ path: entry, base: baseName(entry), outside: isOutside(entry) }));
}

/** Whether an include-style path leaves the profiles directory. */
function isOutside(path: string): boolean {
  return (
    path.startsWith("../") || path.startsWith("..\\") || path.startsWith("/") || /^[A-Za-z]:/.test(path)
  );
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

  private readonly api: GatewayApi;
  private running: EntryData = {};
  private pending: EntryData = {};
  /** Names of models the running profile exposes (the status list). */
  private runningModels: string[] = [];
  /** Unsaved edits: entry key -> field key -> value. */
  private readonly edits = new Map<string, Map<string, unknown>>();
  /** Browser-created entries not yet saved. */
  private drafts: { kind: ModelSource; data: EntryData }[] = [];
  /** Whether the one-time inherited-edit note has been shown. */
  private inheritNoteShown = false;
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
      const [running, pending, dirty, orphans, cache, status] = await Promise.all([
        this.api.getConfig(),
        this.api.getConfigPending(),
        this.api.getConfigDirty(),
        // Headless builds lack /admin/orphans (local feature); the list
        // degrades to empty instead of failing the whole load.
        this.api.getOrphans().catch(() => [] as OrphanFile[]),
        this.api.listCache().catch(() => [] as CacheListEntry[]),
        this.api.getStatus(),
      ]);
      this.running = running;
      this.pending = pending;
      this.dirty = dirty;
      this.orphans = visibleOrphans(orphans);
      this.cache = cache;
      this.activeProfile = status.profile;
      this.runningModels = status.models;
      this.loadError = null;
    } catch (error) {
      this.loadError = error instanceof Error ? error.message : String(error);
    }
    this.loaded = true;
    this.notify();
  }

  /** Re-reads the pending view and the dirty report after a write. */
  private async refreshPending(): Promise<void> {
    const [pending, dirty] = await Promise.all([
      this.api.getConfigPending(),
      this.api.getConfigDirty(),
    ]);
    this.pending = pending;
    this.dirty = dirty;
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
      this.api.getOrphans().catch(() => [] as OrphanFile[]),
      this.api.listCache().catch(() => [] as CacheListEntry[]),
    ]);
    this.orphans = visibleOrphans(orphans);
    this.cache = cache;
  }

  /** The merged model catalog: pending entries plus unsaved drafts. */
  models(): ModelEntry[] {
    const leaf = this.activeProfile ? `${this.activeProfile}.toml` : null;
    const entries: ModelEntry[] = [];
    for (const [array, kind] of [
      ["model", "remote"],
      ["local_model", "local"],
    ] as const) {
      const pendingEntries = this.entriesOf(this.pending, array);
      const runningByName = new Map(
        this.entriesOf(this.running, array).map((entry) => [String(entry["name"] ?? ""), entry]),
      );
      for (const raw of pendingEntries) {
        const data = { ...raw };
        const source = typeof data["source_file"] === "string" ? data["source_file"] : null;
        delete data["source_file"];
        const name = String(data["name"] ?? "");
        const sourceFile = source ? baseName(source) : null;
        // A shadow's provenance names `<leaf>.toml.next`; strip the
        // suffix so the leaf's own shadow never reads as inherited.
        const sourceReal = sourceFile?.replace(/\.next$/, "") ?? null;
        entries.push({
          kind,
          name,
          data,
          sourceFile: sourceReal,
          inherited: sourceReal !== null && leaf !== null && sourceReal !== leaf,
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
        sourceFile: null,
        inherited: false,
        pendingFields: new Set(),
        draft: true,
      });
    }
    return entries;
  }

  /** Finds one entry by kind and name (drafts included). */
  findModel(kind: ModelSource, name: string): ModelEntry | null {
    return this.models().find((entry) => entry.kind === kind && entry.name === name) ?? null;
  }

  /** Finds one entry by name alone, for the `#/models/{name}` route. */
  findByName(name: string): ModelEntry | null {
    return this.models().find((entry) => entry.name === name) ?? null;
  }

  /** Cache metadata for a local model's source, when downloaded. */
  cachedFile(entry: ModelEntry): CacheListEntry | null {
    if (entry.kind !== "local") {
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

  /** The profile's model allowlist, or null when every model is exposed. */
  allowlist(): string[] | null {
    const list = this.pending["models"];
    return Array.isArray(list) ? list.map(String) : null;
  }

  /** The `[[dominion]]` entries of the pending view. */
  dominions(): { id: string; kind: string }[] {
    return this.entriesOf(this.pending, "dominion").map((entry) => ({
      id: String(entry["id"] ?? ""),
      kind: String(entry["kind"] ?? "remote"),
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

  /** The id-keyed entries of one Settings array, with provenance and pending state. */
  keyedEntries(array: "dominion" | "endpoint"): SectionEntry[] {
    const leaf = this.activeProfile ? `${this.activeProfile}.toml` : null;
    const runningById = new Map(
      this.entriesOf(this.running, array).map((entry) => [String(entry["id"] ?? ""), entry]),
    );
    return this.entriesOf(this.pending, array).map((raw) => {
      const data = { ...raw };
      const source = typeof data["source_file"] === "string" ? data["source_file"] : null;
      delete data["source_file"];
      const id = String(data["id"] ?? "");
      const sourceFile = source ? baseName(source) : null;
      const sourceReal = sourceFile?.replace(/\.next$/, "") ?? null;
      return {
        id,
        data,
        sourceFile: sourceReal,
        inherited: sourceReal !== null && leaf !== null && sourceReal !== leaf,
        pendingFields: this.diffFields(data, runningById.get(id)),
      };
    });
  }

  /** The `[[model]]` and `[[local_model]]` entries of the pending view (raw). */
  modelEntriesRaw(): { array: "model" | "local_model"; data: EntryData }[] {
    const rows: { array: "model" | "local_model"; data: EntryData }[] = [];
    for (const array of ["model", "local_model"] as const) {
      for (const data of this.entriesOf(this.pending, array)) {
        rows.push({ array, data });
      }
    }
    return rows;
  }

  /**
   * The full `PUT /admin/config` payload base: the pending view stripped
   * of provenance and of the boot-owned sections. Callers mutate it and
   * pass it to `savePayload`. Untouched secrets are still the `"***"`
   * the gateway sent, so the gateway preserves them, and the pending
   * view's `include` array rides along untouched, so a save keeps the
   * chain explicit - order and membership included.
   */
  buildConfigPayload(): EntryData {
    const payload = structuredClone(this.pending);
    delete payload["source_files"];
    // The boot-owned sections live in gateway.toml, which the profile
    // reaches through its include chain; baking them into the leaf
    // shadow would freeze stale copies the runner later rejects.
    delete payload["server"];
    delete payload["workshop"];
    for (const [array] of KEYED_ARRAYS) {
      for (const item of this.entriesOf(payload, array)) {
        delete item["source_file"];
      }
    }
    return payload;
  }

  /**
   * The `PUT /admin/boot-config` payload base: the boot-owned sections
   * (`[server]`, plus `[workshop]` when present) from the pending view,
   * provenance stripped.
   */
  buildBootPayload(): EntryData {
    const payload: EntryData = {};
    const server = this.pending["server"];
    if (server !== null && typeof server === "object") {
      payload["server"] = structuredClone(server);
    }
    const workshop = this.pending["workshop"];
    if (workshop !== null && typeof workshop === "object") {
      payload["workshop"] = structuredClone(workshop);
    }
    return payload;
  }

  /** Stages a profile payload as the leaf shadow and refreshes. */
  async savePayload(payload: EntryData): Promise<void> {
    await this.api.putConfig(payload);
    await this.refreshPending();
    this.notify();
  }

  /** Stages a boot payload as the boot shadow and refreshes. */
  async saveBootPayload(payload: EntryData): Promise<void> {
    await this.api.putBootConfig(payload);
    await this.refreshPending();
    this.notify();
  }

  /** The active profile's include chain (see {@link includeChainOf}). */
  includeChain(): ChainFile[] {
    return includeChainOf(this.pending);
  }

  /**
   * The pending content attributable to one chain file (by base name):
   * the keyed-array entries whose winning definition it supplied, plus
   * the dotted-path values it last wrote. This is the drill-in editor's
   * base and the `PUT /admin/include/{path}` payload shape. Values the
   * file defines but a later file overrode leave no provenance and are
   * absent, so a drill-in save drops them from the file's shadow.
   */
  includeFileBody(base: string): EntryData {
    const body: EntryData = {};
    for (const [array] of KEYED_ARRAYS) {
      const owned = this.entriesOf(this.pending, array)
        .filter((entry) => {
          const source = entry["source_file"];
          return (
            typeof source === "string" && baseName(source).replace(/\.next$/, "") === base
          );
        })
        .map((entry) => {
          const data = structuredClone(entry);
          delete data["source_file"];
          return data;
        });
      if (owned.length > 0) {
        body[array] = owned;
      }
    }
    const map = this.pending["source_files"];
    if (map !== null && typeof map === "object" && !Array.isArray(map)) {
      const paths = Object.entries(map as EntryData)
        .filter(
          ([, source]) =>
            typeof source === "string" && baseName(source).replace(/\.next$/, "") === base,
        )
        .map(([path]) => path)
        .sort();
      // Sorted, an ancestor path precedes its children; writing the
      // ancestor copies the whole subtree, so children are skipped.
      let written: string | null = null;
      for (const path of paths) {
        if (written !== null && path.startsWith(`${written}.`)) {
          continue;
        }
        const value = readPath(this.pending, path);
        if (value !== undefined) {
          writePath(body, path, structuredClone(value));
          written = path;
        }
      }
    }
    return body;
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
    const array = entry.kind === "local" ? "local_model" : "model";
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

  /**
   * Whether the one-time inherited-edit note is due, and marks it shown:
   * the first edit of an inherited entry per session raises it once.
   */
  takeInheritNote(entry: ModelEntry): boolean {
    if (!entry.inherited || this.inheritNoteShown) {
      return false;
    }
    this.inheritNoteShown = true;
    return true;
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

  /** Stages a new local model directly into the pending config shadow. */
  async stageLocalModel(data: EntryData): Promise<string> {
    const operation = this.stageTail.then(async (): Promise<string> => {
      const payload = this.buildConfigPayload();
      const items = this.entriesOf(payload, "local_model");
      const taken = new Set(this.models().map((entry) => entry.name));
      const base = String(data["name"] ?? "new-local-model");
      let name = base;
      let counter = 2;
      while (taken.has(name)) {
        name = `${base}-${counter}`;
        counter += 1;
      }
      items.push({ ...structuredClone(data), name });
      payload["local_model"] = items;
      const allowlist = payload["models"];
      if (Array.isArray(allowlist)) {
        allowlist.push(name);
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
    const array = entry.kind === "local" ? "local_model" : "model";
    let items = this.entriesOf(payload, array);
    if (entry.draft) {
      items.push(structuredClone(entry.data));
      payload[array] = items;
      return payload;
    }
    if (remove) {
      payload[array] = items.filter((item) => item["name"] !== entry.name);
      return payload;
    }
    const target = items.find((item) => item["name"] === entry.name);
    if (target) {
      const edits = this.edits.get(entryKey(entry));
      for (const [key, value] of edits ?? []) {
        writePath(target, key, value);
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

  /** Deletes every shadow and refreshes. */
  async revertAll(): Promise<void> {
    await this.api.revertConfig();
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
    const isRecord = (value: unknown): value is EntryData =>
      value !== null && typeof value === "object" && !Array.isArray(value);
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
        keys.delete("source_file");
        keys.delete("source_files");
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
    keys.delete("source_file");
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

/** Removes ArtifactStore marker files from a gateway response defensively. */
function visibleOrphans(orphans: OrphanFile[]): OrphanFile[] {
  return orphans.filter((orphan) => !orphan.path.toLocaleLowerCase().endsWith(".verified"));
}
