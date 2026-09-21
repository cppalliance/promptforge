// The mention typeahead: the floating list of chips that opens while
// the operator types a mention in the chat box. One instance lives for
// one suggestion session - the render lifecycle's onStart constructs it
// and onExit disposes it, a pair the suggestion plugin always closes
// (the stopped transition, or the view destroy mid-session) - so the DOM
// and listeners never outlive the session. Positioning is owned by the
// plugin's managed mount(): it appends the popup to document.body,
// anchors it to the live cursor rect, and repositions on scroll and
// resize through Floating UI's autoUpdate; the unmount it returns tears
// all of that down.
//
// The rows are chips: each draws the chip's icon, its label, and its
// `description` dimmed to the right. Items with a `group` are
// ordered by group with a non-selectable header at each boundary;
// keyboard navigation indexes the items only. The plugin's `loading`
// flag renders a loading row while the source is pending. The popup
// writes no fetch, debounce, or staleness logic of its own: the plugin
// supplies the items, the abort, and the flag; the popup only draws.

import "./typeahead-popup.css";

import type { SuggestionKeyDownProps, SuggestionOptions, SuggestionProps } from "@tiptap/suggestion";
import { Disposable, toDisposable } from "../../base/lifecycle";
import { renderChipIcon } from "./chip-view";
import { type ChipNodeAttrs, attrsFromChip } from "./mention-chip";
import type { ChipRef } from "./types";

// The plugin hands the popup ChipRef items and takes the node's
// attributes back through command(): the popup converts at that edge,
// so typeahead-only fields (description, group) never reach the node.
type TypeaheadProps = SuggestionProps<ChipRef, ChipNodeAttrs>;
type TypeaheadRenderer = NonNullable<
  ReturnType<NonNullable<SuggestionOptions<ChipRef, ChipNodeAttrs>["render"]>>
>;

/** One rendered row: a header at a group boundary or a selectable item. */
type Row =
  | { readonly kind: "header"; readonly group: string }
  | { readonly kind: "item"; readonly chip: ChipRef; readonly index: number };

/**
 * Orders the items for display: ungrouped items first in source order,
 * then each group in order of first appearance with its items in source
 * order and a header row ahead of them. `index` counts items only, so
 * it is the selection index.
 */
function layoutRows(items: readonly ChipRef[]): Row[] {
  const rows: Row[] = [];
  let index = 0;
  for (const chip of items) {
    if (chip.group === undefined) {
      rows.push({ kind: "item", chip, index: index++ });
    }
  }
  const groups: string[] = [];
  for (const chip of items) {
    if (chip.group !== undefined && !groups.includes(chip.group)) {
      groups.push(chip.group);
    }
  }
  for (const group of groups) {
    rows.push({ kind: "header", group });
    for (const chip of items) {
      if (chip.group === group) {
        rows.push({ kind: "item", chip, index: index++ });
      }
    }
  }
  return rows;
}

/**
 * The floating suggestion list: a keyboard-navigable <ul> inside a
 * popup <div>. ArrowUp/ArrowDown cycle the highlight over the items
 * with wraparound (headers are skipped), Enter and Tab command the
 * highlighted item, and every other key falls through to the editor.
 * Escape needs no handling here: the plugin dismisses the session on
 * Escape itself, which fires onExit.
 */
export class TypeaheadPopup extends Disposable {
  private readonly element: HTMLDivElement;
  private readonly list: HTMLUListElement;
  /** The selectable items in display order; `selectedIndex` indexes this. */
  private ordered: readonly ChipRef[] = [];
  private loading = false;
  private selectedIndex = 0;
  private command: (chip: ChipRef) => void;

  constructor(props: TypeaheadProps) {
    super();
    this.element = document.createElement("div");
    this.element.className = "ws-typeahead-popup";
    this.list = document.createElement("ul");
    this.list.className = "ws-typeahead-popup__list";
    this.list.setAttribute("role", "listbox");
    this.element.appendChild(this.list);
    // Swallowing the mousedown default keeps the editor's focus and
    // selection when a popup row is clicked.
    this.element.addEventListener("mousedown", (event) => {
      event.preventDefault();
    });
    this.command = (chip) => props.command(attrsFromChip(chip));
    this.applyProps(props);
    // mount() anchors the popup to the cursor rect and repositions it on
    // scroll and resize; the unmount it returns removes the element and
    // every listener mount attached.
    this._register(toDisposable(props.mount(this.element)));
  }

  /**
   * Re-renders for a new props generation: a new query's items, or the
   * loading flag flipping while the source works. No re-anchoring: the
   * mount's rect reader is live, and autoUpdate repositions on scroll
   * and resize.
   */
  update(props: TypeaheadProps): void {
    // command closes over the session's range, so it must be refreshed
    // with every props generation or a stale range would be replaced.
    this.command = (chip) => props.command(attrsFromChip(chip));
    this.applyProps(props);
  }

  /**
   * Handles a keypress while the popup is open. Returns true when the
   * key was consumed; false lets the editor handle it.
   */
  handleKeyDown(props: SuggestionKeyDownProps): boolean {
    const { event } = props;
    if (event.key === "ArrowDown") {
      this.moveSelection(1);
      return true;
    }
    if (event.key === "ArrowUp") {
      this.moveSelection(-1);
      return true;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      const chip = this.ordered[this.selectedIndex];
      if (chip !== undefined) {
        this.command(chip);
      }
      return true;
    }
    if (event.key === "Escape") {
      return true;
    }
    return false;
  }

  private applyProps(props: TypeaheadProps): void {
    const rows = layoutRows(props.items);
    this.ordered = rows.flatMap((row) => (row.kind === "item" ? [row.chip] : []));
    this.loading = props.loading;
    if (this.selectedIndex >= this.ordered.length) {
      this.selectedIndex = 0;
    }
    this.renderRows(rows);
  }

  private moveSelection(delta: number): void {
    const count = this.ordered.length;
    if (count === 0) {
      return;
    }
    this.selectedIndex = (this.selectedIndex + delta + count) % count;
    this.applySelection();
  }

  private renderRows(rows: readonly Row[]): void {
    this.list.textContent = "";
    // A settled query with no matches shows nothing; the session stays
    // alive until the plugin dismisses it. A pending query shows the
    // loading row instead, so the popup does not blink shut mid-search.
    this.element.hidden = rows.length === 0 && !this.loading;
    if (rows.length === 0 && this.loading) {
      const pending = document.createElement("li");
      pending.className = "ws-typeahead-popup__loading";
      pending.setAttribute("role", "presentation");
      pending.textContent = "Searching...";
      this.list.appendChild(pending);
      return;
    }
    for (const row of rows) {
      this.list.appendChild(row.kind === "header" ? renderHeader(row.group) : this.renderItem(row.chip));
    }
    this.applySelection();
  }

  private renderItem(chip: ChipRef): HTMLLIElement {
    const option = document.createElement("li");
    option.className = "ws-typeahead-popup__item";
    option.setAttribute("role", "option");
    if (chip.kind !== undefined) {
      option.setAttribute("data-kind", chip.kind);
    }
    const icon = document.createElement("span");
    icon.className = "ws-typeahead-popup__icon";
    icon.setAttribute("aria-hidden", "true");
    icon.appendChild(renderChipIcon(chip));
    const label = document.createElement("span");
    label.className = "ws-typeahead-popup__label";
    label.textContent = chip.label;
    option.append(icon, label);
    if (chip.description !== undefined) {
      const description = document.createElement("span");
      description.className = "ws-typeahead-popup__description";
      description.textContent = chip.description;
      option.appendChild(description);
    }
    option.addEventListener("click", () => {
      this.command(chip);
    });
    return option;
  }

  private applySelection(): void {
    const options = this.list.querySelectorAll<HTMLLIElement>(".ws-typeahead-popup__item");
    for (let index = 0; index < options.length; index++) {
      const option = options.item(index);
      const selected = index === this.selectedIndex;
      option.classList.toggle("ws-typeahead-popup__item--selected", selected);
      option.setAttribute("aria-selected", selected ? "true" : "false");
    }
  }
}

/** A group boundary: a non-selectable, non-option row naming the group. */
function renderHeader(group: string): HTMLLIElement {
  const header = document.createElement("li");
  header.className = "ws-typeahead-popup__header";
  header.setAttribute("role", "presentation");
  header.textContent = group;
  return header;
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
