// The text-control service: one undo/redo/select-all surface over every
// text-hosting widget. Each widget registers an adapter rooted at its DOM
// subtree (the CodeMirror editor surface, the agent's ProseMirror prompt,
// future Run-panel boxes); the service tracks which root contains focus
// and routes the edit commands to that adapter. When no adapter claims
// focus - a native input, textarea, or contentEditable - the commands
// fall back to document.execCommand on the last focused editable, which
// preserves the native undo stack the way the old window-menu edit
// commands did.
//
// The service owns the document focus tracker (lifted from
// ui/menu/window-menu.ts) and binds the inputFocus, editorTextFocus, and
// textInputFocus context keys, so menus and keybindings see focus state
// from first paint: main.ts resolves the TEXT_CONTROL_SERVICE token at
// boot. The tracker also remembers the last editable target, because
// clicking a menu row moves focus to the menu before the command runs.
//
// DOM-aware but app-free: nothing here may import from the ui/ layers.

import { toDisposable } from "../base/lifecycle";
import type { IDisposable } from "../base/lifecycle";
import { CONTEXT_KEY_SERVICE } from "./context-key-service";
import type { ContextKey, ContextKeyService } from "./context-key-service";
import { createServiceToken, getService, registerService } from "./service-registry";

/**
 * A text-hosting widget's edit surface. `kind` names the widget family
 * ("codemirror", "prosemirror"); editorTextFocus follows the codemirror
 * kind. canUndo/canRedo report history depth so a command on an empty
 * stack falls back to execCommand instead of no-op-ing.
 */
export interface TextControl {
  readonly kind: string;
  undo(): void;
  redo(): void;
  selectAll(): void;
  canUndo?(): boolean;
  canRedo?(): boolean;
}

/** The native text inputs and textareas: what inputFocus means. */
function isTextInput(element: Element | null): element is HTMLElement {
  // The constructor globals are probed: a partial-DOM host (a jsdom test
  // shimming only the keys it needs) may lack them.
  if (typeof HTMLElement === "undefined" || !(element instanceof HTMLElement)) {
    return false;
  }
  if (typeof HTMLTextAreaElement !== "undefined" && element instanceof HTMLTextAreaElement) {
    return !element.disabled && !element.readOnly;
  }
  if (typeof HTMLInputElement !== "undefined" && element instanceof HTMLInputElement) {
    const textLike = ["text", "search", "url", "tel", "email", "password"];
    return !element.disabled && !element.readOnly && textLike.includes(element.type);
  }
  return false;
}

/**
 * Whether the element edits natively, jsdom-compatible: jsdom leaves
 * isContentEditable undefined, so the state comes from the nearest
 * contenteditable ancestor's attribute. An invalid value reads as true
 * rather than the spec's "inherit" - a deliberate simplification.
 */
function isContentEditable(element: HTMLElement): boolean {
  const host = element.closest("[contenteditable]");
  if (host === null) {
    return false;
  }
  return host.getAttribute("contenteditable")?.toLowerCase() !== "false";
}

/** Anything the execCommand fallback can act on natively. */
function isEditable(element: Element | null): element is HTMLElement {
  if (isTextInput(element)) {
    return true;
  }
  return (
    typeof HTMLElement !== "undefined" && element instanceof HTMLElement && isContentEditable(element)
  );
}

/** The page's document, or null where none exists (a DOM-free host). */
function defaultDocument(): Document | null {
  return typeof globalThis.document === "undefined" ? null : globalThis.document;
}

/**
 * The registry of text-control adapters and the owner of the focus
 * context keys. Constructed with the context-key service and the document
 * to track; the default factory resolves both from the service registry
 * and the page globals.
 */
export class TextControlService implements IDisposable {
  private readonly controls = new Map<HTMLElement, TextControl>();
  private readonly inputFocusKey: ContextKey<boolean>;
  private readonly editorTextFocusKey: ContextKey<boolean>;
  private readonly textInputFocusKey: ContextKey<boolean>;
  private readonly doc: Document | null;
  private editTarget: HTMLElement | null = null;

  constructor(contextKeys: ContextKeyService, doc: Document | null = defaultDocument()) {
    this.doc = doc;
    this.inputFocusKey = contextKeys.createKey("inputFocus", false);
    this.editorTextFocusKey = contextKeys.createKey("editorTextFocus", false);
    this.textInputFocusKey = contextKeys.createKey("textInputFocus", false);
    doc?.addEventListener("focusin", this.onFocusIn);
    doc?.addEventListener("focusout", this.onFocusOut);
  }

  /** The adapter whose root contains focus, or null. */
  get active(): TextControl | null {
    return this.controlFor(this.doc?.activeElement ?? null);
  }

  /**
   * Registers `control` as the edit surface for `root`'s subtree.
   * Re-registering the same root replaces the adapter; the returned
   * disposable unregisters only its own registration.
   */
  register(root: HTMLElement, control: TextControl): IDisposable {
    this.controls.set(root, control);
    this.recompute();
    return toDisposable(() => {
      if (this.controls.get(root) === control) {
        this.controls.delete(root);
        this.recompute();
      }
    });
  }

  /** Undo on the active adapter, or the native fallback. */
  undo(): void {
    const active = this.active;
    if (active !== null && (active.canUndo === undefined || active.canUndo())) {
      active.undo();
      return;
    }
    this.runFallback("undo");
  }

  /** Redo on the active adapter, or the native fallback. */
  redo(): void {
    const active = this.active;
    if (active !== null && (active.canRedo === undefined || active.canRedo())) {
      active.redo();
      return;
    }
    this.runFallback("redo");
  }

  /** Select-all on the active adapter, or the native fallback. */
  selectAll(): void {
    const active = this.active;
    if (active !== null) {
      active.selectAll();
      return;
    }
    this.runFallback("selectAll");
  }

  /**
   * Cut/copy/paste: always the native path, on the remembered editable.
   * CodeMirror and ProseMirror serve the command through the clipboard
   * events the execCommand fires; native editables keep their own
   * semantics.
   */
  execCommand(command: "cut" | "copy" | "paste"): void {
    this.runFallback(command);
  }

  private readonly onFocusIn = (event: FocusEvent): void => {
    const target = event.target instanceof Element ? event.target : null;
    if (isEditable(target)) {
      this.editTarget = target;
    }
    this.recompute();
  };

  private readonly onFocusOut = (event: FocusEvent): void => {
    // Focus moving to another element recomputes on that element's
    // focusin; focus leaving the page (no related target) clears here.
    if (event.relatedTarget === null) {
      this.recompute(null);
    }
  };

  /** The adapter whose root contains `element`, or null. */
  private controlFor(element: Element | null): TextControl | null {
    if (element === null) {
      return null;
    }
    for (const [root, control] of this.controls) {
      if (root.contains(element)) {
        return control;
      }
    }
    return null;
  }

  /**
   * Re-evaluates the three focus keys against `element` - the document's
   * active element by default, null when focus left the page.
   */
  private recompute(element: Element | null = this.doc?.activeElement ?? null): void {
    const active = this.controlFor(element);
    this.editorTextFocusKey.set(active !== null && active.kind === "codemirror");
    this.inputFocusKey.set(isTextInput(element));
    this.textInputFocusKey.set(active !== null || isEditable(element));
  }

  /**
   * The native path: refocus the last editable element (a menu click has
   * moved focus since) and execCommand, which preserves the editable's
   * native undo stack. A disconnected or absent target no-ops, as does a
   * document without execCommand (jsdom).
   */
  private runFallback(command: string): void {
    const target = this.editTarget;
    if (target === null || !target.isConnected) {
      return;
    }
    target.focus();
    if (this.doc !== null && typeof this.doc.execCommand === "function") {
      this.doc.execCommand(command);
    }
  }

  dispose(): void {
    this.doc?.removeEventListener("focusin", this.onFocusIn);
    this.doc?.removeEventListener("focusout", this.onFocusOut);
  }
}

/** The registry token for the text-control singleton. */
export const TEXT_CONTROL_SERVICE = createServiceToken<TextControlService>("workshop.textControl");

// Self-registration: the default instance is shared by every consumer in
// the process. main.ts resolves the token at boot so the focus keys exist
// from first paint.
registerService(TEXT_CONTROL_SERVICE, () => new TextControlService(getService(CONTEXT_KEY_SERVICE)));
