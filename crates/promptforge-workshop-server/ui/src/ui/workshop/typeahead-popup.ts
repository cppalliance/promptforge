// The mention typeahead: the floating list of @-file suggestions that
// opens while the operator types a mention in the prompt input. One
// instance lives for one suggestion session - the render lifecycle's
// onStart constructs it and onExit disposes it, a pair the suggestion
// plugin always closes (the stopped transition, or the view destroy
// mid-session) - so the DOM and listeners never outlive the session.
// Positioning is owned by the plugin's managed mount(): it appends the
// popup to document.body, anchors it to the live cursor rect, and
// repositions on scroll and resize through Floating UI's autoUpdate;
// the unmount it returns tears all of that down. The item source is a
// stub of three canned entries standing in for the workspace file index
// until one exists.

import "./typeahead-popup.css";

import type { MentionOptions } from "@tiptap/extension-mention";
import { Disposable, toDisposable } from "../../base/lifecycle";

/** One row in the mention typeahead: an @-referenced workspace file. */
export interface MentionItem {
  readonly id: string;
  readonly label: string;
}

// STUB for the future workspace file index: three canned entries keep
// the popup's open/filter/select cycle testable until the index exists.
const STUB_ITEMS: readonly MentionItem[] = [
  { id: "README.md", label: "README.md" },
  { id: "src/main.ts", label: "src/main.ts" },
  { id: "Cargo.toml", label: "Cargo.toml" },
];

/**
 * The item source wired into the mention suggestion: the stub entries
 * filtered by case-insensitive substring match on the query.
 */
export function mentionTypeaheadItems(query: string): MentionItem[] {
  const needle = query.toLowerCase();
  return STUB_ITEMS.filter((item) => item.label.toLowerCase().includes(needle));
}

// The lifecycle prop types are derived from the mention extension's own
// options, so they track the installed suggestion plugin without a
// direct dependency on @tiptap/suggestion.
type MentionSuggestion = MentionOptions<MentionItem>["suggestion"];
type TypeaheadRenderer = NonNullable<ReturnType<NonNullable<MentionSuggestion["render"]>>>;
type TypeaheadProps = Parameters<NonNullable<TypeaheadRenderer["onStart"]>>[0];
type TypeaheadKeyDownProps = Parameters<NonNullable<TypeaheadRenderer["onKeyDown"]>>[0];

/**
 * The floating suggestion list: a keyboard-navigable <ul> inside a
 * popup <div>. ArrowUp/ArrowDown cycle the highlight with wraparound,
 * Enter commands the highlighted item, and every other key falls
 * through to the editor. Escape needs no handling here: the plugin
 * dismisses the session on Escape itself, which fires onExit.
 */
export class TypeaheadPopup extends Disposable {
  private readonly element: HTMLDivElement;
  private readonly list: HTMLUListElement;
  private items: readonly MentionItem[];
  private selectedIndex = 0;
  private command: (item: MentionItem) => void;

  constructor(props: TypeaheadProps) {
    super();
    this.element = document.createElement("div");
    this.element.className = "typeahead-popup";
    this.list = document.createElement("ul");
    this.list.className = "typeahead-popup__list";
    this.list.setAttribute("role", "listbox");
    this.element.appendChild(this.list);
    // Swallowing the mousedown default keeps the editor's focus and
    // selection when a popup row is clicked.
    this.element.addEventListener("mousedown", (event) => {
      event.preventDefault();
    });
    this.command = props.command;
    this.items = props.items;
    this.renderItems();
    // mount() anchors the popup to the cursor rect and repositions it on
    // scroll and resize; the unmount it returns removes the element and
    // every listener mount attached.
    this._register(toDisposable(props.mount(this.element)));
  }

  /**
   * Re-filters and re-renders for the new query. No re-anchoring: the
   * mount's rect reader is live, and autoUpdate repositions on scroll
   * and resize.
   */
  update(props: TypeaheadProps): void {
    // command closes over the session's range, so it must be refreshed
    // with every props generation or a stale range would be replaced.
    this.command = props.command;
    this.items = props.items;
    if (this.selectedIndex >= this.items.length) {
      this.selectedIndex = 0;
    }
    this.renderItems();
  }

  /**
   * Handles a keypress while the popup is open. Returns true when the
   * key was consumed; false lets the editor handle it.
   */
  handleKeyDown(props: TypeaheadKeyDownProps): boolean {
    const { event } = props;
    if (event.key === "ArrowDown") {
      this.moveSelection(1);
      return true;
    }
    if (event.key === "ArrowUp") {
      this.moveSelection(-1);
      return true;
    }
    if (event.key === "Enter") {
      const item = this.items[this.selectedIndex];
      if (item !== undefined) {
        this.command(item);
      }
      return true;
    }
    if (event.key === "Escape") {
      return true;
    }
    return false;
  }

  private moveSelection(delta: number): void {
    const count = this.items.length;
    if (count === 0) {
      return;
    }
    this.selectedIndex = (this.selectedIndex + delta + count) % count;
    this.applySelection();
  }

  private renderItems(): void {
    this.list.textContent = "";
    // A query with no matches shows nothing; the session stays alive
    // until the plugin dismisses it.
    this.element.hidden = this.items.length === 0;
    for (const item of this.items) {
      const option = document.createElement("li");
      option.className = "typeahead-popup__item";
      option.setAttribute("role", "option");
      option.textContent = item.label;
      option.addEventListener("click", () => {
        this.command(item);
      });
      this.list.appendChild(option);
    }
    this.applySelection();
  }

  private applySelection(): void {
    for (let index = 0; index < this.list.children.length; index++) {
      const child = this.list.children.item(index);
      if (child === null) {
        continue;
      }
      const selected = index === this.selectedIndex;
      child.classList.toggle("typeahead-popup__item--selected", selected);
      child.setAttribute("aria-selected", selected ? "true" : "false");
    }
  }
}

/**
 * The suggestion render lifecycle: one TypeaheadPopup per session,
 * constructed on onStart and disposed on onExit.
 */
export function renderMentionTypeahead(): TypeaheadRenderer {
  let popup: TypeaheadPopup | undefined;
  return {
    onStart: (props) => {
      popup = new TypeaheadPopup(props);
    },
    onUpdate: (props) => {
      popup?.update(props);
    },
    onKeyDown: (props) => popup?.handleKeyDown(props) ?? false,
    onExit: () => {
      popup?.dispose();
      popup = undefined;
    },
  };
}
