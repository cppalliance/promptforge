// The config store: running config, pending (shadow-overlaid) config,
// the dirty report, and the browser-side edit state. The three states
// the write path defines: *dirty* is an unsaved edit held here, keyed
// per model and field; *pending* is a saved shadow, visible as the
// difference between the pending and running views; *applied* is the
// running config itself. Save builds the full PUT /admin/config payload
// from the pending view plus one model's edits - untouched secrets ride
// through as the "***" the gateway sent, so no real secret ever leaves
// or re-enters the browser.

import type { DirtyReport, GatewayApi, OrphanFile } from "./gateway-api";

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

/** One include-chain file derived from the pending view's provenance. */
export interface ChainFile {
  /** The include-style path relative to the profiles dir (`common.toml`, `../gateway.toml`). */
  path: string;
  /** The file's base name (`common.toml`, `gateway.toml`). */
  base: string;
  /** Whether the file lives outside the profiles directory. */
  outside: boolean;
}

/** A provenance path normalized: `/` separators, `.next` suffix stripped. */
function normalizeSource(source: string): string {
  return source.replace(/\\/g, "/").replace(/\.next$/, "");
}

/** Every distinct provenance source path in a pending view, normalized. */
function provenanceSources(pending: EntryData): string[] {
  const sources = new Set<string>();
  const map = pending["source_files"];
  if (map !== null && typeof map === "object" && !Array.isArray(map)) {
    for (const value of Object.values(map as EntryData)) {
      if (typeof value === "string") {
        sources.add(normalizeSource(value));
      }
    }
  }
  for (const array of ["dominion", "endpoint", "model", "local_model"]) {
    const entries = pending[array];
    if (!Array.isArray(entries)) {
      continue;
    }
    for (const entry of entries) {
      if (entry !== null && typeof entry === "object") {
        const source = (entry as EntryData)["source_file"];
        if (typeof source === "string") {
          sources.add(normalizeSource(source));
        }
      }
    }
  }
  return [...sources];
}

/**
 * Derives the active profile's include chain from the pending view's
 * provenance: the distinct files the merge visited, minus the leaf
 * itself. Provenance is a per-path last-writer map, so two limits hold
 * and the chain editor states them: the merge ORDER is not recoverable
 * (rows sort boot-file-first, then alphabetically, until a save writes
 * an explicit `include` array), and a chain file whose every value was
 * overridden later leaves no provenance and does not appear.
 */
export function deriveIncludeChain(pending: EntryData, activeProfile: string): ChainFile[] {
  const leafBase = `${activeProfile}.toml`;
  const sources = provenanceSources(pending);
  // The profiles directory is the leaf's own directory, when any value
  // is attributed to the leaf; otherwise paths degrade to base names.
  const leafPath = sources.find((source) => baseName(source) === leafBase) ?? null;
  const profilesDir = leafPath === null ? null : leafPath.slice(0, leafPath.lastIndexOf("/"));
  const files: ChainFile[] = [];
  for (const source of sources) {
    const base = baseName(source);
    if (base === leafBase) {
      continue;
    }
    files.push(chainFile(source, base, profilesDir));
  }
  files.sort((a, b) =>
    a.outside === b.outside ? a.path.localeCompare(b.path) : a.outside ? -1 : 1,
  );
  return files;
}

/** One chain row: the include-relative path for an absolute source. */
function chainFile(source: string, base: string, profilesDir: string | null): ChainFile {
  if (profilesDir === null) {
    // No leaf attribution to anchor on; a bare base name still names
    // profile-dir files correctly, the overwhelmingly common case.
    return { path: base, base, outside: false };
  }
  if (source.startsWith(`${profilesDir}/`)) {
    return { path: source.slice(profilesDir.length + 1), base, outside: false };
  }
  // Walk up from the profiles dir to the common prefix, then down.
  const dirParts = profilesDir.split("/");
  const sourceParts = source.split("/");
  let shared = 0;
  while (
    shared < dirParts.length &&
    shared < sourceParts.length - 1 &&
    dirParts[shared] === sourceParts[shared]
  ) {
    shared += 1;
  }
  const ups = "../".repeat(dirParts.length - shared);
  return { path: ups + sourceParts.slice(shared).join("/"), base, outside: true };
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
      const [running, pending, dirty, orphans, status] = await Promise.all([
        this.api.getConfig(),
        this.api.getConfigPending(),
        this.api.getConfigDirty(),
        // Headless builds lack /admin/orphans (local feature); the list
        // degrades to empty instead of failing the whole load.
        this.api.getOrphans().catch(() => [] as OrphanFile[]),
        this.api.getStatus(),
      ]);
      this.running = running;
      this.pending = pending;
      this.dirty = dirty;
      this.orphans = orphans;
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
    await this.refreshPending();
  }

  /** Re-reads the orphan list (after a cache delete). */
  async refreshOrphans(): Promise<void> {
    this.orphans = await this.api.getOrphans().catch(() => [] as OrphanFile[]);
    this.notify();
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
   * the gateway sent, so the gateway preserves them.
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

  /** The active profile's derived include chain (see {@link deriveIncludeChain}). */
  includeChain(): ChainFile[] {
    return deriveIncludeChain(this.pending, this.activeProfile);
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
