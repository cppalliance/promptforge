// The editor settings service: the four user-facing editor toggles
// (word wrap, render whitespace, render control characters, column
// selection), seeded from the UI-state adapter's user bucket at
// construction and written back through it on every change so they
// survive a relaunch, and published as the config.editor.* context keys
// so the menus' `toggled` expressions follow them. EditorSurface
// subscribes to onDidChange and reconfigures one Compartment per setting;
// the toggle actions in editor.contribution.ts call toggle().
//
// The persisted value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as the defaults, never
// as a cast. The writer is fire-and-forget: a write that throws is
// swallowed and the in-memory values stay authoritative.
//
// The service self-registers with a default factory (empty initial,
// no-op writer), so any bundle that touches it gets a working singleton;
// the composition root re-registers it bound to the live adapter before
// the first consumer resolves it.
//
// DOM-free: the initial value and the writer are injected.

import { Emitter } from "../../base/event";
import type { Event } from "../../base/event";
import { ContextKeyService, CONTEXT_KEY_SERVICE } from "../../services/context-key-service";
import type { ContextKey } from "../../services/context-key-service";
import {
  DEFAULT_EDITOR_SETTINGS,
  EDITOR_SETTING_CONTEXT_KEYS,
  EDITOR_SETTINGS_SERVICE,
  type EditorSettingName,
  type EditorSettings,
  type EditorSettingsService as EditorSettingsServiceContract,
} from "../../services/editor-settings-service";
import { getServiceOrNull, registerService } from "../../services/service-registry";

/** The setting names, in declaration order. */
const SETTING_NAMES = [
  "wordWrap",
  "renderWhitespace",
  "renderControlCharacters",
  "columnSelection",
] as const satisfies readonly EditorSettingName[];

/** The writer the service hands each new settings object to. */
export type EditorSettingsWriter = (value: unknown) => void;

/**
 * Narrows a persisted payload to EditorSettings: it must be a plain
 * object, each known key keeps its value only when it is a boolean, and
 * missing or mistyped keys fall back to the defaults. Anything else
 * (null, an array, a string, a number) reads as the defaults.
 */
function readSettings(initial: unknown): EditorSettings {
  if (typeof initial !== "object" || initial === null || Array.isArray(initial)) {
    return DEFAULT_EDITOR_SETTINGS;
  }
  const record: Record<string, unknown> = initial as Record<string, unknown>;
  const settings = { ...DEFAULT_EDITOR_SETTINGS };
  for (const name of SETTING_NAMES) {
    const value: unknown = record[name];
    if (typeof value === "boolean") {
      settings[name] = value;
    }
  }
  return settings;
}

/**
 * The editor settings. Mutations write through eagerly, update the
 * config.editor.* context keys, and fire onDidChange with the new
 * settings; a writer failure leaves the in-memory values authoritative
 * for the rest of the page lifetime.
 */
export class EditorSettingsService implements EditorSettingsServiceContract {
  private current: EditorSettings;
  private readonly changeEmitter = new Emitter<EditorSettings>();
  private readonly keys: { [K in EditorSettingName]?: ContextKey<boolean> } = {};

  /** Fires when a setting changes; the editor surfaces hook it. */
  readonly onDidChange: Event<EditorSettings> = this.changeEmitter.event;

  /**
   * `initial` is the value the user bucket held at boot (any shape; see
   * readSettings); `write` receives the whole settings object after each
   * change.
   */
  constructor(
    initial: unknown = null,
    private readonly write: EditorSettingsWriter = () => {},
    contextKeys: ContextKeyService | null = getServiceOrNull(CONTEXT_KEY_SERVICE),
  ) {
    this.current = readSettings(initial);
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

  private persist(): void {
    try {
      this.write(this.current);
    } catch {
      // The adapter reports its own failures; a throwing writer leaves
      // the in-memory values authoritative.
    }
  }

  dispose(): void {
    this.changeEmitter.dispose();
  }
}

// Self-registration with the defaults and a no-op writer: a consumer that
// resolves the token before the composition root re-registers it bound to
// the live adapter gets working, unpersisted settings rather than a wrong
// instance cached for the page lifetime.
registerService(EDITOR_SETTINGS_SERVICE, () => new EditorSettingsService());
