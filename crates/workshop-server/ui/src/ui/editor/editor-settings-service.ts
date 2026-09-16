// The editor settings service: the four user-facing editor toggles
// (word wrap, render whitespace, render control characters, column
// selection), persisted to localStorage so they survive a reload and
// published as the config.editor.* context keys so the menus' `toggled`
// expressions follow them. EditorSurface subscribes to onDidChange and
// reconfigures one Compartment per setting; the toggle actions in
// editor.contribution.ts call toggle().
//
// The persisted value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as the defaults, never
// as a cast. Storage access itself can throw (denied access, quota), so
// every read and write is guarded and the service degrades to in-memory.
//
// The service self-registers with a default factory, so any bundle that
// touches it gets the singleton without composition-root wiring.
//
// DOM-free: storage is injectable and defaults to the page's
// localStorage when one exists.

import { Emitter } from "../../base/event";
import type { Event } from "../../base/event";
import type { IDisposable } from "../../base/lifecycle";
import { ContextKeyService, CONTEXT_KEY_SERVICE } from "../../services/context-key-service";
import type { ContextKey } from "../../services/context-key-service";
import { createServiceToken, getServiceOrNull, registerService } from "../../services/service-registry";

/** The four editor settings, one boolean per toggle action. */
export interface EditorSettings {
  readonly wordWrap: boolean;
  readonly renderWhitespace: boolean;
  readonly renderControlCharacters: boolean;
  readonly columnSelection: boolean;
}

/** One setting's name. */
export type EditorSettingName = keyof EditorSettings;

/**
 * The stock values. Render Control Characters is on by default, as in
 * Cursor; the other three start off.
 */
export const DEFAULT_EDITOR_SETTINGS: EditorSettings = {
  wordWrap: false,
  renderWhitespace: false,
  renderControlCharacters: true,
  columnSelection: false,
};

/** The setting names, in declaration order. */
const SETTING_NAMES = [
  "wordWrap",
  "renderWhitespace",
  "renderControlCharacters",
  "columnSelection",
] as const satisfies readonly EditorSettingName[];

/** The context key each setting publishes to (the menus' `toggled` sources). */
export const EDITOR_SETTING_CONTEXT_KEYS: { readonly [K in EditorSettingName]: `config.editor.${K}` } = {
  wordWrap: "config.editor.wordWrap",
  renderWhitespace: "config.editor.renderWhitespace",
  renderControlCharacters: "config.editor.renderControlCharacters",
  columnSelection: "config.editor.columnSelection",
};

/** The localStorage key holding the persisted settings object. */
const STORAGE_KEY = "workshop.editorSettings";

/**
 * Narrows a persisted payload to EditorSettings: it must be a JSON
 * object, each known key keeps its value only when it is a boolean, and
 * missing or mistyped keys fall back to the defaults. Anything else
 * reads as the defaults.
 */
function readSettings(raw: string | null): EditorSettings {
  if (raw === null) {
    return DEFAULT_EDITOR_SETTINGS;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return DEFAULT_EDITOR_SETTINGS;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return DEFAULT_EDITOR_SETTINGS;
  }
  const record: Record<string, unknown> = parsed as Record<string, unknown>;
  const settings = { ...DEFAULT_EDITOR_SETTINGS };
  for (const name of SETTING_NAMES) {
    const value: unknown = record[name];
    if (typeof value === "boolean") {
      settings[name] = value;
    }
  }
  return settings;
}

/** The page's localStorage, or null where none exists (a DOM-free host). */
function defaultStorage(): Storage | null {
  try {
    return typeof globalThis.localStorage === "undefined" ? null : globalThis.localStorage;
  } catch {
    return null;
  }
}

/**
 * The editor settings. Mutations persist eagerly, update the
 * config.editor.* context keys, and fire onDidChange with the new
 * settings; a storage failure leaves the in-memory values authoritative
 * for the rest of the page lifetime.
 */
export class EditorSettingsService implements IDisposable {
  private current: EditorSettings;
  private readonly changeEmitter = new Emitter<EditorSettings>();
  private readonly keys: { [K in EditorSettingName]?: ContextKey<boolean> } = {};

  /** Fires when a setting changes; the editor surfaces hook it. */
  readonly onDidChange: Event<EditorSettings> = this.changeEmitter.event;

  constructor(
    private readonly storage: Storage | null = defaultStorage(),
    private readonly storageKey: string = STORAGE_KEY,
    contextKeys: ContextKeyService | null = getServiceOrNull(CONTEXT_KEY_SERVICE),
  ) {
    this.current = readSettings(this.readRaw());
    if (contextKeys !== null) {
      for (const name of SETTING_NAMES) {
        const key = contextKeys.createKey<boolean>(EDITOR_SETTING_CONTEXT_KEYS[name], DEFAULT_EDITOR_SETTINGS[name]);
        this.keys[name] = key;
        // The declared default is already visible through getValue; only
        // a persisted override needs a write (and its change event).
        if (this.current[name] !== DEFAULT_EDITOR_SETTINGS[name]) {
          key.set(this.current[name]);
        }
      }
    }
  }

  /** The current settings. */
  get settings(): EditorSettings {
    return this.current;
  }

  /** Writes one setting; a write of the current value is a no-op. */
  set(name: EditorSettingName, value: boolean): void {
    if (this.current[name] === value) {
      return;
    }
    this.current = { ...this.current, [name]: value };
    this.persist();
    this.keys[name]?.set(value);
    this.changeEmitter.fire(this.current);
  }

  /** Flips one setting - the toggle actions' entire run body. */
  toggle(name: EditorSettingName): void {
    this.set(name, !this.current[name]);
  }

  private readRaw(): string | null {
    if (this.storage === null) {
      return null;
    }
    try {
      return this.storage.getItem(this.storageKey);
    } catch {
      return null;
    }
  }

  private persist(): void {
    if (this.storage === null) {
      return;
    }
    try {
      this.storage.setItem(this.storageKey, JSON.stringify(this.current));
    } catch {
      // Quota or denied access: the in-memory values stay authoritative.
    }
  }

  dispose(): void {
    this.changeEmitter.dispose();
  }
}

/** The registry token for the editor-settings singleton. */
export const EDITOR_SETTINGS_SERVICE = createServiceToken<EditorSettingsService>("workshop.editorSettings");

// Self-registration: the default instance is shared by every consumer in
// the process. The composition root may re-register to rebind.
registerService(EDITOR_SETTINGS_SERVICE, () => new EditorSettingsService());
