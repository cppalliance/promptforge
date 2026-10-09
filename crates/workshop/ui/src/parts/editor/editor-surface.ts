// The EditorSurface contract and its CodeMirror 6 implementation. The
// surface owns everything editor-concrete: the EditorView, the extension
// set, the theme, lazy language modes, and dirty tracking. Panels, zones,
// and the save flow are written against EditorSurface only - nothing else
// in the app imports @codemirror/* directly. Dirty tracking is an
// updateListener comparing the live document against the last opened or
// saved text; markSaved takes the exact text a write persisted, so
// keystrokes that land while the write is in flight stay dirty.
// Runtime-reconfigurables (language, readOnly, and the four editor
// settings) sit behind Compartments on the surface; the externalUpdate
// annotation marks server-originated reloads so listeners can tell them
// from local typing. The base extension set is basicSetup spelled out,
// because three of its members - highlightSpecialChars,
// rectangularSelection, and highlightWhitespace - move into settings
// compartments, and a bundle cannot be picked apart.
//
// The look and the gestures copy Cursor's editor (Monaco's stock
// defaults with the minimap off): the theme reads the --editor-* tokens
// from @workshop/look, the highlight style reads --editor-token-*, the
// cursor is a 2px bar blinking once a second, a plain Alt+click adds a
// cursor and Shift+Alt+drag selects a column (no crosshair), the indent
// unit is four spaces, the document scrolls past its last line, and the
// find widget floats at the top right (find-widget.ts).

import {
  Decoration,
  type DecorationSet,
  drawSelection,
  dropCursor,
  EditorView,
  gutter,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  highlightWhitespace,
  keymap,
  lineNumbers,
  rectangularSelection,
  scrollPastEnd,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import {
  Annotation,
  Compartment,
  EditorState,
  type Extension,
  type Range,
  type Transaction,
} from "@codemirror/state";
import { defaultKeymap, history, historyKeymap, redo, redoDepth, selectAll, undo, undoDepth } from "@codemirror/commands";
import {
  bracketMatching,
  codeFolding,
  defaultHighlightStyle,
  foldGutter,
  foldKeymap,
  HighlightStyle,
  indentOnInput,
  indentUnit,
  StreamLanguage,
  syntaxHighlighting,
} from "@codemirror/language";
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { search, searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { lintKeymap } from "@codemirror/lint";
import { tags } from "@lezer/highlight";

import { ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT } from "@workshop/look/icons";
import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import { getServiceOrNull } from "@workshop/platform/service-registry";
import { TEXT_CONTROL_SERVICE } from "@workshop/platform/text-control-service";
import {
  DEFAULT_EDITOR_SETTINGS,
  EDITOR_SETTINGS_SERVICE,
  type EditorSettings,
  type EditorSettingsService,
  type RenderWhitespace,
} from "../../services/editor-settings-service";
import { createFindWidget } from "./find-widget";

/** A document handed to the surface: the path it came from and its text. */
export interface EditorDocument {
  readonly path: string;
  readonly text: string;
}

/**
 * Tags transactions whose content came from the server - workspace file
 * loads and reloads - rather than from local typing, so autosave-like
 * listeners can avoid writing back text the server just sent. Check it
 * with {@link isExternalUpdate}.
 */
export const externalUpdate = Annotation.define<boolean>();

/** Whether a transaction holds server-originated content (see {@link externalUpdate}). */
export function isExternalUpdate(tr: Transaction): boolean {
  return tr.annotation(externalUpdate) === true;
}

/**
 * The editor abstraction the rest of the workshop is written against.
 * Implementations own one document at a time; open replaces it.
 */
export interface EditorSurface {
  /** The root element the panel mounts. */
  readonly element: HTMLElement;
  /** Loads a document, replacing any current one and resetting dirty state. */
  open(document: EditorDocument): void;
  /** The current editor text - what a save would write. */
  text(): string;
  /**
   * Records the text a successful write persisted as the saved baseline
   * and recomputes dirty against the live document, so edits made while
   * the write was in flight stay dirty.
   */
  markSaved(text: string): void;
  /** Whether the text differs from the last open or markSaved baseline. */
  isDirty(): boolean;
  /** Toggles read-only mode; document, history, and view state survive. */
  setReadOnly(readOnly: boolean): void;
  /** Registers a listener fired on dirty-state transitions; returns an unsubscribe. */
  onDirtyChange(listener: (dirty: boolean) => void): () => void;
  /**
   * The live EditorView, or null before the first open(). Exposed for
   * the editor directory's command layer (editor-commands.ts); nothing
   * outside the editor directory should consume it.
   */
  editorView(): EditorView | null;
  focus(): void;
  dispose(): void;
}

// The dark theme skins from the --editor-* tokens in @workshop/look/tokens.css,
// Cursor Dark's editor values. Text is 14px on a 19px line, set on the
// scroller so the line numbers share it. CodeMirror's own dark theme is
// overridden where it is more specific than a short selector: the focused
// selection needs the long .cm-selectionLayer path to win, and the bracket
// match must show with the editor unfocused, which the built-in rule does
// not.
const promptforgeTheme = EditorView.theme(
  {
    "&": {
      backgroundColor: "var(--editor-bg)",
      color: "var(--editor-fg)",
      height: "100%",
    },
    "&.cm-focused": {
      outline: "none",
    },
    ".cm-scroller": {
      fontFamily: "var(--code-font)",
      fontSize: "var(--editor-font-size)",
      lineHeight: "var(--editor-line-height)",
    },
    ".cm-content": {
      caretColor: "var(--editor-cursor)",
      padding: "0",
    },
    ".cm-line": {
      padding: "0",
    },
    // A 2px bar, shifted 1px left so it sits between the characters.
    ".cm-cursor, .cm-dropCursor": {
      borderLeftWidth: "var(--editor-cursor-width)",
      borderLeftColor: "var(--editor-cursor)",
      marginLeft: "var(--editor-cursor-shift)",
    },
    // The gutter reads glyph lane, line numbers, fold lane. No right border
    // and no tint on the active line's gutter; only its number brightens.
    ".cm-gutters": {
      backgroundColor: "var(--editor-gutter-bg)",
      color: "var(--editor-line-number)",
      border: "none",
      borderRight: "none",
    },
    ".cm-glyphMargin": {
      minWidth: "var(--editor-glyph-margin)",
    },
    ".cm-lineNumbers .cm-gutterElement": {
      minWidth: "var(--editor-line-number-min-width)",
      padding: "0",
    },
    ".cm-foldGutter": {
      minWidth: "var(--editor-fold-lane)",
    },
    ".cm-activeLineGutter": {
      backgroundColor: "transparent",
      color: "var(--editor-line-number-active)",
    },
    // The current line's wash gives way to a selection (the has-selection
    // class rides on the editor through editorAttributes).
    ".cm-activeLine": {
      backgroundColor: "var(--editor-current-line)",
    },
    "&.ws-has-selection .cm-activeLine": {
      backgroundColor: "transparent",
    },
    ".cm-selectionBackground": {
      background: "var(--editor-selection-inactive)",
      borderRadius: "var(--editor-selection-radius)",
    },
    "& > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
      background: "var(--editor-selection-inactive)",
      borderRadius: "var(--editor-selection-radius)",
    },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
      background: "var(--editor-selection)",
      borderRadius: "var(--editor-selection-radius)",
    },
    ".cm-searchMatch": {
      backgroundColor: "var(--editor-find-match)",
      outline: "none",
    },
    ".cm-searchMatch.cm-searchMatch-selected": {
      backgroundColor: "var(--editor-find-match-current)",
    },
    // The word under the cursor, and the other occurrences of a selection.
    ".cm-selectionMatch": {
      backgroundColor: "var(--editor-selection-highlight)",
    },
    ".cm-selectionMatch.cm-selectionMatch-main": {
      backgroundColor: "var(--editor-word-highlight)",
    },
    ".cm-matchingBracket, &.cm-focused .cm-matchingBracket": {
      backgroundColor: "var(--editor-bracket-match)",
      border: "var(--ws-border-width) solid transparent",
    },
    // Fold chevrons fade in while the pointer is over the gutter; a folded
    // range's chevron stays. The folded range itself reads as a bare ellipsis.
    ".cm-foldGutter .ws-fold-marker": {
      display: "inline-flex",
      color: "var(--editor-fold-chevron)",
      opacity: "0",
      transition: "opacity var(--editor-fold-fade)",
    },
    ".cm-gutters:hover .ws-fold-marker, .cm-foldGutter .ws-fold-marker[data-state=folded]": {
      opacity: "1",
    },
    "@media (prefers-reduced-motion: reduce)": {
      ".cm-foldGutter .ws-fold-marker": {
        transition: "none",
      },
    },
    ".cm-foldPlaceholder": {
      backgroundColor: "transparent",
      border: "none",
      color: "var(--editor-fold-placeholder)",
    },
    ".cm-highlightSpace": {
      backgroundImage: "radial-gradient(circle at 50% 55%, var(--editor-whitespace) 20%, transparent 5%)",
    },
    ".cm-panels": {
      backgroundColor: "transparent",
      color: "var(--editor-fg)",
    },
  },
  { dark: true },
);

// Syntax colors are Cursor Dark's tokenColors, by lezer tag, read from the
// --editor-token-* values in @workshop/look/tokens.css. A tag falls back
// to its parent's rule (controlKeyword to keyword, integer to number), so
// each rule names the family.
const promptforgeHighlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier], color: "var(--editor-token-keyword)" },
  { tag: tags.operator, color: "var(--editor-token-operator)" },
  { tag: tags.punctuation, color: "var(--editor-token-punctuation)" },
  { tag: tags.angleBracket, color: "var(--editor-token-tag-punctuation)" },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--editor-token-string)" },
  { tag: [tags.regexp, tags.escape], color: "var(--editor-token-punctuation)" },
  { tag: tags.number, color: "var(--editor-token-number)" },
  { tag: [tags.bool, tags.null, tags.atom, tags.self], color: "var(--editor-token-constant)" },
  { tag: tags.comment, color: "var(--editor-token-comment)", fontStyle: "italic" },
  { tag: [tags.typeName, tags.className, tags.namespace], color: "var(--editor-token-type)" },
  {
    tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName],
    color: "var(--editor-token-function)",
  },
  { tag: [tags.variableName, tags.definition(tags.variableName)], color: "var(--editor-token-variable)" },
  { tag: tags.constant(tags.variableName), color: "var(--editor-token-constant-variable)" },
  { tag: tags.propertyName, color: "var(--editor-token-property)" },
  { tag: tags.attributeName, color: "var(--editor-token-attribute)" },
  { tag: tags.tagName, color: "var(--editor-token-tag)" },
  { tag: tags.heading, color: "var(--editor-token-heading)", fontWeight: "bold" },
  { tag: [tags.link, tags.url], color: "var(--editor-token-link)", textDecoration: "underline" },
  { tag: tags.emphasis, color: "var(--editor-token-emphasis)", fontStyle: "italic" },
  { tag: tags.strong, color: "var(--editor-token-strong)", fontWeight: "bold" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: tags.invalid, color: "var(--editor-token-invalid)" },
]);

// The has-selection class: on the editor while any range is non-empty, so
// the stylesheet can hide the current-line wash under a selection.
const hasSelectionClass = EditorView.editorAttributes.compute(
  ["selection"],
  (state): Record<string, string> => (state.selection.ranges.some((range) => !range.empty) ? { class: "ws-has-selection" } : {}),
);

/** A fold lane chevron: down while the range is open, right once folded. */
function foldMarker(open: boolean): HTMLElement {
  const marker = document.createElement("span");
  marker.className = "ws-fold-marker";
  marker.dataset.state = open ? "open" : "folded";
  marker.innerHTML = open ? ICON_CHEVRON_DOWN : ICON_CHEVRON_RIGHT;
  return marker;
}

/** The lowercase file extension of a path, or null when it has none. */
function extensionOf(path: string): string | null {
  const name = path.split(/[\\/]/).filter(Boolean).pop();
  if (name === undefined) {
    return null;
  }
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? null : name.slice(dot + 1).toLowerCase();
}

/** One language mode: its editorLangId and its lazy extension loader. */
interface LanguageMode {
  readonly id: string;
  readonly load: () => Promise<Extension>;
}

async function javascriptMode(typescript: boolean, jsx: boolean): Promise<Extension> {
  const { javascript } = await import("@codemirror/lang-javascript");
  return javascript({ typescript, jsx });
}

/**
 * The language modes by file extension. First-party packs cover
 * JavaScript/TypeScript, Python, Rust, JSON, Markdown, and YAML; TOML
 * comes through the legacy-modes stream parser. Unknown extensions get
 * plain text. (The single-file esbuild bundle inlines these dynamic
 * imports; the structure keeps the load boundary explicit.) The id is
 * the value the editorLangId context key publishes.
 */
const LANGUAGE_MODES: Record<string, LanguageMode> = {
  js: { id: "javascript", load: () => javascriptMode(false, true) },
  mjs: { id: "javascript", load: () => javascriptMode(false, true) },
  cjs: { id: "javascript", load: () => javascriptMode(false, true) },
  jsx: { id: "javascriptreact", load: () => javascriptMode(false, true) },
  ts: { id: "typescript", load: () => javascriptMode(true, false) },
  mts: { id: "typescript", load: () => javascriptMode(true, false) },
  cts: { id: "typescript", load: () => javascriptMode(true, false) },
  tsx: { id: "typescriptreact", load: () => javascriptMode(true, true) },
  py: { id: "python", load: async () => (await import("@codemirror/lang-python")).python() },
  rs: { id: "rust", load: async () => (await import("@codemirror/lang-rust")).rust() },
  json: { id: "json", load: async () => (await import("@codemirror/lang-json")).json() },
  md: { id: "markdown", load: async () => (await import("@codemirror/lang-markdown")).markdown() },
  markdown: { id: "markdown", load: async () => (await import("@codemirror/lang-markdown")).markdown() },
  yaml: { id: "yaml", load: async () => (await import("@codemirror/lang-yaml")).yaml() },
  yml: { id: "yaml", load: async () => (await import("@codemirror/lang-yaml")).yaml() },
  toml: {
    id: "toml",
    load: async () => StreamLanguage.define((await import("@codemirror/legacy-modes/mode/toml")).toml),
  },
};

/** The editorLangId for a path's extension; "plaintext" when it has no mode. */
export function languageIdForPath(path: string): string {
  const extension = extensionOf(path);
  if (extension === null) {
    return "plaintext";
  }
  return LANGUAGE_MODES[extension]?.id ?? "plaintext";
}

/** The language mode for a path, loaded on demand; null for plain text. */
async function languageFor(path: string): Promise<Extension | null> {
  const extension = extensionOf(path);
  if (extension === null) {
    return null;
  }
  return (await LANGUAGE_MODES[extension]?.load()) ?? null;
}

// EditorState.readOnly gates commands and transaction filters;
// EditorView.editable controls the DOM contenteditable attribute. A
// user-facing read-only toggle needs both.
function readOnlyExtension(readOnly: boolean): Extension {
  return [EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)];
}

/** Word wrap: lineWrapping when on, nothing when off. */
function wordWrapExtension(on: boolean): Extension {
  return on ? EditorView.lineWrapping : [];
}

const tabMark = Decoration.mark({ class: "cm-highlightTab" });
const spaceMark = Decoration.mark({ class: "cm-highlightSpace" });

/** Marks the spaces and tabs inside every non-empty selection range, within the visible text. */
function markSelectedWhitespace(view: EditorView): DecorationSet {
  const marks: Range<Decoration>[] = [];
  for (const range of view.state.selection.ranges) {
    if (range.empty) {
      continue;
    }
    for (const visible of view.visibleRanges) {
      const from = Math.max(range.from, visible.from);
      const to = Math.min(range.to, visible.to);
      if (from >= to) {
        continue;
      }
      const text = view.state.sliceDoc(from, to);
      for (let index = 0; index < text.length; index += 1) {
        const char = text[index];
        if (char === "\t") {
          marks.push(tabMark.range(from + index, from + index + 1));
        } else if (char === " ") {
          marks.push(spaceMark.range(from + index, from + index + 1));
        }
      }
    }
  }
  return Decoration.set(marks, true);
}

/** The selection mode's decorator: the same marks highlightWhitespace uses, scoped to the selection. */
const selectionWhitespace = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = markSelectedWhitespace(view);
    }

    update(update: ViewUpdate): void {
      if (update.docChanged || update.selectionSet || update.viewportChanged) {
        this.decorations = markSelectedWhitespace(update.view);
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/**
 * Render Whitespace: nothing for none, the cm-highlightSpace and
 * cm-highlightTab marks inside the selection only for selection, and
 * the stock decorator over the whole document for all.
 */
function whitespaceExtension(mode: RenderWhitespace): Extension {
  switch (mode) {
    case "none":
      return [];
    case "selection":
      return selectionWhitespace;
    case "all":
      return highlightWhitespace();
  }
}

/** Render Control Characters: the cm-specialChar decorator when on. */
function controlCharactersExtension(on: boolean): Extension {
  return on ? highlightSpecialChars() : [];
}

/**
 * Column Selection Mode: on makes every left drag rectangular; off keeps
 * Cursor's gesture, Shift+Alt+drag, because a plain Alt+click adds a
 * cursor instead.
 */
function columnSelectionExtension(on: boolean): Extension {
  return rectangularSelection({
    eventFilter: on ? () => true : (event) => event.altKey && event.shiftKey && event.button === 0,
  });
}

/** The CodeMirror 6 EditorSurface. */
export class CodeMirrorSurface extends Disposable implements EditorSurface {
  readonly element = document.createElement("div");
  private view: EditorView | null = null;
  private savedText = "";
  private dirty = false;
  private readonly listeners = new Set<(dirty: boolean) => void>();
  // Everything reconfigurable at runtime sits behind a Compartment on the
  // surface: a change is one reconfigure dispatch, never a state rebuild
  // that would drop history, selection, and scroll position.
  private readonly language = new Compartment();
  private readonly readOnly = new Compartment();
  private readonly wordWrap = new Compartment();
  private readonly whitespace = new Compartment();
  private readonly controlCharacters = new Compartment();
  private readonly columnSelection = new Compartment();
  private readOnlyState = false;
  private settings: EditorSettings;
  // Discards a lazy language load that resolves after a newer open().
  private openGeneration = 0;

  constructor(settingsService: EditorSettingsService | null = getServiceOrNull(EDITOR_SETTINGS_SERVICE)) {
    super();
    this.element.className = "ws-editor-surface";
    // The surface follows the editor settings service: the current values
    // seed the first state, and every change reconfigures the live view.
    this.settings = settingsService?.settings ?? DEFAULT_EDITOR_SETTINGS;
    if (settingsService !== null) {
      this._register(
        settingsService.onDidChange((next) => {
          this.applySettings(next);
        }),
      );
    }
    // The surface is its own text-control adapter: the Edit menu's
    // undo/redo/select-all route here whenever this subtree holds focus.
    // The view is lazy, so the adapter guards against a pre-open state.
    const textControls = getServiceOrNull(TEXT_CONTROL_SERVICE);
    if (textControls !== null) {
      this._register(
        textControls.register(this.element, {
          kind: "codemirror",
          undo: () => {
            if (this.view !== null) {
              undo(this.view);
            }
          },
          redo: () => {
            if (this.view !== null) {
              redo(this.view);
            }
          },
          selectAll: () => {
            if (this.view !== null) {
              selectAll(this.view);
            }
          },
          canUndo: () => this.view !== null && undoDepth(this.view.state) > 0,
          canRedo: () => this.view !== null && redoDepth(this.view.state) > 0,
        }),
      );
    }
    // The view is created lazily by open(), so its teardown is registered
    // here once, against whichever view is live at dispose time.
    this._register(
      toDisposable(() => {
        this.view?.destroy();
        this.view = null;
        this.listeners.clear();
      }),
    );
  }

  open(document: EditorDocument): void {
    this.openGeneration += 1;
    const generation = this.openGeneration;
    this.savedText = document.text;
    if (this.view === null) {
      this.view = new EditorView({
        parent: this.element,
        state: EditorState.create({
          doc: document.text,
          extensions: [
            // basicSetup spelled out, minus highlightSpecialChars,
            // rectangularSelection, and highlightWhitespace, which live in
            // settings compartments, and minus crosshairCursor, which Cursor
            // does not draw. The glyph lane comes before the line numbers.
            gutter({ class: "cm-glyphMargin" }),
            lineNumbers(),
            highlightActiveLineGutter(),
            history(),
            codeFolding({ placeholderText: "\u22EF" }),
            foldGutter({ markerDOM: foldMarker }),
            drawSelection({ cursorBlinkRate: 1000 }),
            dropCursor(),
            EditorState.allowMultipleSelections.of(true),
            // Alt+click adds a cursor.
            EditorView.clickAddsSelectionRange.of((event) => event.altKey && !event.shiftKey),
            indentUnit.of("    "),
            scrollPastEnd(),
            indentOnInput(),
            syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
            bracketMatching(),
            closeBrackets(),
            autocompletion(),
            highlightActiveLine(),
            highlightSelectionMatches({ highlightWordAroundCursor: true }),
            keymap.of([
              ...closeBracketsKeymap,
              ...defaultKeymap,
              ...searchKeymap,
              ...historyKeymap,
              ...foldKeymap,
              ...completionKeymap,
              ...lintKeymap,
            ]),
            search({ createPanel: createFindWidget }),
            hasSelectionClass,
            promptforgeTheme,
            syntaxHighlighting(promptforgeHighlight),
            this.language.of([]),
            this.readOnly.of(readOnlyExtension(this.readOnlyState)),
            this.wordWrap.of(wordWrapExtension(this.settings.wordWrap)),
            this.whitespace.of(whitespaceExtension(this.settings.renderWhitespace)),
            this.controlCharacters.of(controlCharactersExtension(this.settings.renderControlCharacters)),
            this.columnSelection.of(columnSelectionExtension(this.settings.columnSelection)),
            EditorView.updateListener.of((update) => {
              if (update.docChanged) {
                this.setDirty(update.state.doc.toString() !== this.savedText);
              }
            }),
          ],
        }),
      });
    } else {
      // A reload lands in the live view as one annotated transaction, not
      // a state rebuild: compartments and history survive, and listeners
      // (a future autosave) can tell the server-originated replacement
      // from local typing by the annotation.
      this.view.dispatch({
        changes: { from: 0, to: this.view.state.doc.length, insert: document.text },
        annotations: externalUpdate.of(true),
      });
    }
    this.setDirty(false);
    void languageFor(document.path)
      .then((mode) => {
        if (generation === this.openGeneration && this.view !== null) {
          // A null mode still reconfigures - to empty - because a reload
          // keeps the view alive, so the previous document's mode must be
          // cleared rather than left in the compartment.
          this.view.dispatch({ effects: this.language.reconfigure(mode ?? []) });
        }
      })
      .catch(() => {
        // A failed language load leaves the document as plain text.
      });
  }

  setReadOnly(readOnly: boolean): void {
    if (readOnly === this.readOnlyState) {
      return;
    }
    this.readOnlyState = readOnly;
    this.view?.dispatch({ effects: this.readOnly.reconfigure(readOnlyExtension(readOnly)) });
  }

  /**
   * Applies new editor settings: one reconfigure dispatch across the
   * four settings compartments, so history, selection, and scroll
   * position survive. Stored for the first open() when no view exists.
   */
  private applySettings(settings: EditorSettings): void {
    this.settings = settings;
    this.view?.dispatch({
      effects: [
        this.wordWrap.reconfigure(wordWrapExtension(settings.wordWrap)),
        this.whitespace.reconfigure(whitespaceExtension(settings.renderWhitespace)),
        this.controlCharacters.reconfigure(controlCharactersExtension(settings.renderControlCharacters)),
        this.columnSelection.reconfigure(columnSelectionExtension(settings.columnSelection)),
      ],
    });
  }

  text(): string {
    return this.view?.state.doc.toString() ?? "";
  }

  editorView(): EditorView | null {
    return this.view;
  }

  markSaved(text: string): void {
    this.savedText = text;
    this.setDirty(this.text() !== text);
  }

  isDirty(): boolean {
    return this.dirty;
  }

  onDirtyChange(listener: (dirty: boolean) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  focus(): void {
    this.view?.focus();
  }

  private setDirty(dirty: boolean): void {
    if (dirty === this.dirty) {
      return;
    }
    this.dirty = dirty;
    for (const listener of this.listeners) {
      listener(dirty);
    }
  }
}
