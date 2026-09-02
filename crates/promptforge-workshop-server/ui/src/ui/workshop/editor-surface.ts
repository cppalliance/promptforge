// The EditorSurface contract and its CodeMirror 6 implementation. The
// surface owns everything editor-concrete: the EditorView, the extension
// set, the theme, lazy language modes, and dirty tracking. Panels, zones,
// and the save flow are written against EditorSurface only - nothing else
// in the app imports @codemirror/* directly. Dirty tracking is an
// updateListener comparing the live document against the last opened or
// saved text; markSaved takes the exact text a write persisted, so
// keystrokes that land while the write is in flight stay dirty.
// Runtime-reconfigurables (language, readOnly) sit behind Compartments on
// the surface; the externalUpdate annotation marks server-originated
// reloads so listeners can tell them from local typing.

import { basicSetup } from "codemirror";
import {
  Annotation,
  Compartment,
  EditorState,
  type Extension,
  type Transaction,
} from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { search } from "@codemirror/search";
import { HighlightStyle, StreamLanguage, syntaxHighlighting } from "@codemirror/language";
import { tags } from "@lezer/highlight";

import { Disposable, toDisposable } from "../../base/lifecycle";

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

/** Whether a transaction carries server-originated content (see {@link externalUpdate}). */
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
  focus(): void;
  dispose(): void;
}

// The dark theme skins from the same :root tokens as the rest of the UI;
// every var() carries the token's stock value as fallback.
const promptforgeTheme = EditorView.theme(
  {
    "&": {
      backgroundColor: "var(--bg)",
      color: "var(--text)",
      height: "100%",
      fontSize: "13px",
    },
    ".cm-content": {
      fontFamily: 'var(--code-font)',
      caretColor: "var(--text)",
    },
    ".cm-cursor, .cm-dropCursor": {
      borderLeftColor: "var(--text)",
    },
    ".cm-gutters": {
      backgroundColor: "var(--bg-raised)",
      color: "var(--text-muted)",
      border: "none",
      borderRight: "1px solid var(--border)",
    },
    // Not --bg-hover: its #252525 wash drops accent-colored syntax tokens
    // to 4.14:1, under the 4.5:1 floor. A 4% white wash over --bg keeps
    // every syntax color at or above 4.5:1 on the active line.
    ".cm-activeLineGutter": {
      backgroundColor: "rgba(255, 255, 255, 0.04)",
    },
    ".cm-activeLine": {
      backgroundColor: "rgba(255, 255, 255, 0.04)",
    },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground": {
      backgroundColor: "var(--accent-dim)",
    },
    "&.cm-focused": {
      outline: "1px solid var(--accent-dim)",
      outlineOffset: "-1px",
    },
    ".cm-searchMatch": {
      backgroundColor: "var(--accent-dim)",
      outline: "1px solid var(--accent)",
    },
    ".cm-searchMatch-selected": {
      backgroundColor: "var(--accent)",
    },
    ".cm-panels": {
      backgroundColor: "var(--bg-raised)",
      color: "var(--text)",
    },
    ".cm-panels input, .cm-panels button": {
      backgroundColor: "var(--bg)",
      color: "var(--text)",
      border: "1px solid var(--border)",
      borderRadius: "var(--radius)",
    },
  },
  { dark: true },
);

// Syntax colors drawn from the palette: accent for keywords and headings,
// the LED green/amber for strings and literals, muted gray for comments
// and punctuation. Every value stays at or above 4.5:1 on --bg.
const promptforgeHighlight = HighlightStyle.define([
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword], color: "var(--accent)" },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--led-green)" },
  { tag: [tags.number, tags.bool, tags.atom, tags.null], color: "var(--led-amber)" },
  { tag: [tags.comment, tags.blockComment], color: "var(--text-muted)", fontStyle: "italic" },
  { tag: [tags.typeName, tags.className, tags.tagName], color: "var(--accent)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName)], color: "var(--text)" },
  { tag: [tags.propertyName, tags.attributeName], color: "var(--text)" },
  { tag: [tags.operator, tags.punctuation, tags.separator], color: "var(--text-muted)" },
  { tag: tags.heading, color: "var(--accent)", fontWeight: "bold" },
  { tag: tags.link, color: "var(--accent)", textDecoration: "underline" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strong, fontWeight: "bold" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: tags.invalid, color: "var(--danger-text)" },
]);

/** The lowercase file extension of a path, or null when it has none. */
function extensionOf(path: string): string | null {
  const name = path.split(/[\\/]/).filter(Boolean).pop();
  if (name === undefined) {
    return null;
  }
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? null : name.slice(dot + 1).toLowerCase();
}

/**
 * The language mode for a file extension, loaded on demand. First-party
 * packs cover JavaScript/TypeScript, Python, Rust, JSON, Markdown, and
 * YAML; TOML comes through the legacy-modes stream parser. Unknown
 * extensions get plain text. (The single-file esbuild bundle inlines
 * these dynamic imports; the structure keeps the load boundary explicit.)
 */
async function languageFor(path: string): Promise<Extension | null> {
  switch (extensionOf(path)) {
    case "js":
    case "mjs":
    case "cjs":
    case "jsx": {
      const { javascript } = await import("@codemirror/lang-javascript");
      return javascript({ jsx: true });
    }
    case "ts":
    case "mts":
    case "cts": {
      const { javascript } = await import("@codemirror/lang-javascript");
      return javascript({ typescript: true });
    }
    case "tsx": {
      const { javascript } = await import("@codemirror/lang-javascript");
      return javascript({ typescript: true, jsx: true });
    }
    case "py": {
      const { python } = await import("@codemirror/lang-python");
      return python();
    }
    case "rs": {
      const { rust } = await import("@codemirror/lang-rust");
      return rust();
    }
    case "json": {
      const { json } = await import("@codemirror/lang-json");
      return json();
    }
    case "md":
    case "markdown": {
      const { markdown } = await import("@codemirror/lang-markdown");
      return markdown();
    }
    case "yaml":
    case "yml": {
      const { yaml } = await import("@codemirror/lang-yaml");
      return yaml();
    }
    case "toml": {
      const { toml } = await import("@codemirror/legacy-modes/mode/toml");
      return StreamLanguage.define(toml);
    }
    default:
      return null;
  }
}

// EditorState.readOnly gates commands and transaction filters;
// EditorView.editable controls the DOM contenteditable attribute. A
// user-facing read-only toggle needs both.
function readOnlyExtension(readOnly: boolean): Extension {
  return [EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)];
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
  private readOnlyState = false;
  // Discards a lazy language load that resolves after a newer open().
  private openGeneration = 0;

  constructor() {
    super();
    this.element.className = "editor-surface";
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
            basicSetup,
            search(),
            promptforgeTheme,
            syntaxHighlighting(promptforgeHighlight),
            this.language.of([]),
            this.readOnly.of(readOnlyExtension(this.readOnlyState)),
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

  text(): string {
    return this.view?.state.doc.toString() ?? "";
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
