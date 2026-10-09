// The editor settings vocabulary and service contract. The implementation
// (parts/editor/editor-settings-service.ts) stays CodeMirror-free and
// self-registers with a default factory, but it lives in parts beside the
// surfaces that consume it; the settings shape, the context-key mapping,
// the defaults, the service interface, and the token live here, in the
// DOM-free services layer.

import type { Event } from "@workshop/platform/event";
import type { IDisposable } from "@workshop/platform/lifecycle";
import { createServiceToken, type ServiceToken } from "@workshop/platform/service-registry";

/**
 * What Render Whitespace draws: nothing, the whitespace inside the
 * selection only (Cursor's default), or every space and tab.
 */
export type RenderWhitespace = "none" | "selection" | "all";

/** The render-whitespace modes, in menu order. */
export const RENDER_WHITESPACE_MODES: readonly RenderWhitespace[] = ["none", "selection", "all"];

/**
 * The editor settings: three on/off toggles and the render-whitespace
 * mode. The toggle actions flip the booleans; Render Whitespace's toggle
 * flips between none and all.
 */
export interface EditorSettings {
  readonly wordWrap: boolean;
  readonly renderWhitespace: RenderWhitespace;
  readonly renderControlCharacters: boolean;
  readonly columnSelection: boolean;
}

/** One setting's name. */
export type EditorSettingName = keyof EditorSettings;

/**
 * The stock values. Render Control Characters is on by default, as in
 * Cursor; Render Whitespace defaults to the selection mode, as in
 * Cursor; the other two start off.
 */
export const DEFAULT_EDITOR_SETTINGS: EditorSettings = {
  wordWrap: false,
  renderWhitespace: "selection",
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

/**
 * The boolean a setting publishes to its context key. Every key is
 * boolean, so the Render Whitespace menu row can show a check: the mode
 * publishes true unless it is none.
 */
export function contextKeyValue<K extends EditorSettingName>(name: K, value: EditorSettings[K]): boolean {
  return name === "renderWhitespace" ? value !== "none" : value === true;
}

/** The editor-settings service consumers resolve from the registry. */
export interface EditorSettingsService extends IDisposable {
  /** The current settings. */
  readonly settings: EditorSettings;
  /** Fires when a setting changes; the editor surfaces hook it. */
  readonly onDidChange: Event<EditorSettings>;
  /** Writes one setting; a write of the current value is a no-op. */
  set<K extends EditorSettingName>(name: K, value: EditorSettings[K]): void;
  /**
   * Flips one setting - the toggle actions' entire run body. A boolean
   * flips; Render Whitespace flips between none and all.
   */
  toggle(name: EditorSettingName): void;
}

/** The registry token for the editor-settings singleton. */
export const EDITOR_SETTINGS_SERVICE = createServiceToken<EditorSettingsService>("workshop.editorSettings");
