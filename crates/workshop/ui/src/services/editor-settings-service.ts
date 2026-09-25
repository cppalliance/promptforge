// The editor settings vocabulary and service contract. The implementation
// (parts/editor/editor-settings-service.ts) stays CodeMirror-free and
// self-registers with a default factory, but it lives in parts beside the
// surfaces that consume it; the settings shape, the context-key mapping,
// the defaults, the service interface, and the token live here, in the
// DOM-free services layer.

import type { Event } from "../base/event";
import type { IDisposable } from "../base/lifecycle";
import { createServiceToken, type ServiceToken } from "./service-registry";

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

/** The context key each setting publishes to (the menus' `toggled` sources). */
export const EDITOR_SETTING_CONTEXT_KEYS: { readonly [K in EditorSettingName]: `config.editor.${K}` } = {
  wordWrap: "config.editor.wordWrap",
  renderWhitespace: "config.editor.renderWhitespace",
  renderControlCharacters: "config.editor.renderControlCharacters",
  columnSelection: "config.editor.columnSelection",
};

/** The editor-settings service consumers resolve from the registry. */
export interface EditorSettingsService extends IDisposable {
  /** The current settings. */
  readonly settings: EditorSettings;
  /** Fires when a setting changes; the editor surfaces hook it. */
  readonly onDidChange: Event<EditorSettings>;
  /** Writes one setting; a write of the current value is a no-op. */
  set(name: EditorSettingName, value: boolean): void;
  /** Flips one setting - the toggle actions' entire run body. */
  toggle(name: EditorSettingName): void;
}

/** The registry token for the editor-settings singleton. */
export const EDITOR_SETTINGS_SERVICE = createServiceToken<EditorSettingsService>("workshop.editorSettings");
