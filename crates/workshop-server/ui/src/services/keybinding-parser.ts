// The keybinding parser: turns chord strings ("ctrl+m ctrl+o") into
// structured Chord values and maps KeyboardEvents onto the same shape.
// The modifier vocabulary is VS Code's: ctrl, shift, alt, meta (alias
// cmd), and ctrlcmd, which resolves to meta on macOS and ctrl everywhere
// else, so one catalog row binds Cmd+S on macOS and Ctrl+S elsewhere
// without a per-OS table. The key vocabulary is letters, digits,
// f1-f12, the named editing and navigation keys, numpad0-numpad9, and
// punctuation by unshifted glyph.
//
// A KeyboardEvent maps through event.code, never event.key: the code
// names the physical key, so Ctrl+Shift+= and Ctrl+= both read as "="
// and Numpad0 stays distinct from Digit0.
//
// Parsing returns a Result - a malformed string is a value the registrar
// reports once at registration, never an exception thrown at dispatch.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { err, ok, type Result } from "./error-catalog";
import type { ParseError } from "./context-key-expr";

/** The platforms keybindings resolve against. */
export type KeybindingPlatform = "windows" | "mac" | "linux";

/**
 * The running platform, from the browser's navigator when present.
 * Plain-node tests and unknown environments resolve to windows, the
 * platform whose chords the catalog's keybinding column shows.
 */
export function detectPlatform(): KeybindingPlatform {
  const nav = typeof navigator === "undefined" ? undefined : navigator;
  const text = `${nav?.platform ?? ""} ${nav?.userAgent ?? ""}`.toLowerCase();
  if (text.includes("mac")) {
    return "mac";
  }
  if (text.includes("linux")) {
    return "linux";
  }
  return "windows";
}

/**
 * One pressed chord: the modifier state plus a key from the vocabulary.
 * The key is stored in its canonical lowercase form ("s", "0", "f5",
 * "numpad0", "pagedown", "=").
 */
export interface Chord {
  readonly ctrl: boolean;
  readonly shift: boolean;
  readonly alt: boolean;
  readonly meta: boolean;
  readonly key: string;
}

/** The named (non-letter, non-digit, non-punctuation) key vocabulary. */
const NAMED_KEYS: ReadonlySet<string> = new Set([
  ...Array.from({ length: 12 }, (_, i) => `f${i + 1}`),
  "enter",
  "escape",
  "tab",
  "space",
  "backspace",
  "delete",
  "insert",
  "home",
  "end",
  "pageup",
  "pagedown",
  "up",
  "down",
  "left",
  "right",
  ...Array.from({ length: 10 }, (_, i) => `numpad${i}`),
]);

/** Punctuation keys, named by their unshifted glyph. */
const PUNCTUATION_KEYS: ReadonlySet<string> = new Set(["=", "-", "[", "]", "\\", ";", "'", ",", ".", "/", "`"]);

function isKeyToken(token: string): boolean {
  if (token.length === 1) {
    const ch = token.charAt(0);
    return (ch >= "a" && ch <= "z") || (ch >= "0" && ch <= "9") || PUNCTUATION_KEYS.has(token);
  }
  return NAMED_KEYS.has(token);
}

/** Internal control flow for parse failures; caught at the parse boundary. */
class ParseFailure extends Error {
  constructor(readonly parseError: ParseError) {
    super(parseError.message);
  }
}

function fail(message: string, offset: number): never {
  throw new ParseFailure({ message, offset });
}

function parseChord(text: string, offset: number, platform: KeybindingPlatform): Chord {
  const parts = text.split("+");
  const key = parts[parts.length - 1] ?? "";
  if (key === "" || !isKeyToken(key)) {
    fail(`unknown key '${key}'`, offset);
  }
  let ctrl = false;
  let shift = false;
  let alt = false;
  let meta = false;
  for (const part of parts.slice(0, -1)) {
    switch (part) {
      case "ctrl":
        ctrl = true;
        break;
      case "shift":
        shift = true;
        break;
      case "alt":
        alt = true;
        break;
      case "meta":
      case "cmd":
        meta = true;
        break;
      case "ctrlcmd":
        // VS Code's CtrlCmd: Cmd on macOS, Ctrl everywhere else.
        if (platform === "mac") {
          meta = true;
        } else {
          ctrl = true;
        }
        break;
      default:
        fail(`unknown modifier '${part}'`, offset);
    }
  }
  return { ctrl, shift, alt, meta, key };
}

/**
 * Parses a whitespace-separated chord string ("ctrl+m ctrl+o") into
 * Chord values. Modifier and key tokens are case-insensitive; spaces
 * inside a chord are not allowed. `ctrlcmd` resolves against `platform`,
 * which defaults to the detected platform. Returns a ParseError value on
 * malformed input.
 */
export function parseKeybinding(text: string, platform: KeybindingPlatform = detectPlatform()): Result<readonly Chord[], ParseError> {
  try {
    const trimmed = text.trim();
    if (trimmed === "") {
      fail("expected a keybinding", 0);
    }
    const chords: Chord[] = [];
    let offset = 0;
    for (const chordText of trimmed.split(/\s+/)) {
      const at = text.indexOf(chordText, offset);
      chords.push(parseChord(chordText.toLowerCase(), at === -1 ? offset : at, platform));
      offset = (at === -1 ? offset : at) + chordText.length;
    }
    return ok(chords);
  } catch (failure) {
    if (failure instanceof ParseFailure) {
      return err(failure.parseError);
    }
    throw failure;
  }
}

/** The event.code to key-vocabulary mapping for non-letter keys. */
const CODE_TO_KEY: Readonly<Record<string, string>> = {
  Enter: "enter",
  Escape: "escape",
  Tab: "tab",
  Space: "space",
  Backspace: "backspace",
  Delete: "delete",
  Insert: "insert",
  Home: "home",
  End: "end",
  PageUp: "pageup",
  PageDown: "pagedown",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Equal: "=",
  Minus: "-",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backquote: "`",
};

function keyFromCode(code: string): string | undefined {
  if (/^Key[A-Z]$/.test(code)) {
    return code.slice(3).toLowerCase();
  }
  if (/^Digit[0-9]$/.test(code)) {
    return code.slice(5);
  }
  if (/^Numpad[0-9]$/.test(code)) {
    return code.toLowerCase();
  }
  if (/^F([1-9]|1[0-2])$/.test(code)) {
    return code.toLowerCase();
  }
  return CODE_TO_KEY[code];
}

/**
 * Maps a KeyboardEvent to a Chord through event.code, never event.key:
 * the code names the physical key, so Ctrl+Shift+= and Ctrl+= both read
 * as "=" and Numpad0 stays distinct from Digit0. Modifier-only presses
 * and unmapped codes answer undefined.
 */
export function chordFromKeyboardEvent(event: KeyboardEvent): Chord | undefined {
  const key = keyFromCode(event.code);
  if (key === undefined) {
    return undefined;
  }
  return { ctrl: event.ctrlKey, shift: event.shiftKey, alt: event.altKey, meta: event.metaKey, key };
}

/** Structural chord equality. */
export function chordsEqual(a: Chord, b: Chord): boolean {
  return a.ctrl === b.ctrl && a.shift === b.shift && a.alt === b.alt && a.meta === b.meta && a.key === b.key;
}

const KEY_LABELS: Readonly<Record<string, string>> = {
  enter: "Enter",
  escape: "Escape",
  tab: "Tab",
  space: "Space",
  backspace: "Backspace",
  delete: "Delete",
  insert: "Insert",
  home: "Home",
  end: "End",
  pageup: "PageUp",
  pagedown: "PageDown",
  up: "Up",
  down: "Down",
  left: "Left",
  right: "Right",
};

function keyLabel(key: string): string {
  if (key.length === 1 && key >= "a" && key <= "z") {
    return key.toUpperCase();
  }
  if (key.startsWith("numpad")) {
    return `NumPad${key.slice(6)}`;
  }
  return KEY_LABELS[key] ?? key;
}

/**
 * Renders a chord for display: "Ctrl+M" on Windows and Linux, "Cmd+S"
 * for meta chords on macOS. Modifiers render in Ctrl, Shift, Alt, Meta
 * order.
 */
export function formatChord(chord: Chord, platform: KeybindingPlatform = detectPlatform()): string {
  const parts: string[] = [];
  if (chord.ctrl) {
    parts.push("Ctrl");
  }
  if (chord.shift) {
    parts.push("Shift");
  }
  if (chord.alt) {
    parts.push("Alt");
  }
  if (chord.meta) {
    parts.push(platform === "mac" ? "Cmd" : "Meta");
  }
  parts.push(keyLabel(chord.key));
  return parts.join("+");
}

/** Renders a chord sequence for display: "Ctrl+M Ctrl+O". */
export function formatKeybinding(chords: readonly Chord[], platform: KeybindingPlatform = detectPlatform()): string {
  return chords.map((chord) => formatChord(chord, platform)).join(" ");
}
