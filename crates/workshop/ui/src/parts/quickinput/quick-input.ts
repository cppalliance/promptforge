// The quick input widget: one floating panel under the title bar that
// routes its input to the quick-access registry's providers by longest
// prefix and renders the active provider's items as a WAI-ARIA combobox
// (an input role=combobox with aria-autocomplete=list, aria-expanded,
// aria-controls, and aria-activedescendant, a visually-hidden label, and
// a ul role=listbox of li role=option rows). ArrowUp/ArrowDown move the
// active row, Enter accepts it, Escape dismisses the panel and returns
// focus to whatever held it before the panel opened.
//
// Routing is in place: typing a character that changes the longest
// matching prefix swaps the provider, the placeholder, and the rows
// without closing the panel, so "debug foo" reaches the debug provider
// while "" still owns every unprefixed value. The filter handed to a
// provider is the input value minus the prefix. A provider may redirect
// the value to another mode (the help provider does, for "?>").
//
// Rows draw Cursor's pieces: the label with its filter match
// highlighted, the muted description, and the keybinding as key chips (one
// per key, "+" between, a gap between chords). A group label (a row's
// `separator`) is a presentational line above the row it opens and never
// takes the active row. A provider that answers no rows and has a
// noResultsMessage shows it as an inert line.
//
// The modes list (includeHelp, shown while the input is empty) is the
// help entries that carry a commandCenterOrder, in that order, labelled
// by commandCenterLabel when they have one, each with its prefix as the
// description and its command's keybinding.
//
// Providers are narrowed, never cast: the registry stores factories
// returning unknown, and a factory whose product lacks getItems renders
// an empty list rather than throwing at open.

import "./quick-input.css";

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import { KeybindingsRegistry } from "@workshop/platform/keybinding-registry";
import { QuickAccessRegistry } from "@workshop/platform/quick-access-registry";
import {
  type LabelHighlight,
  type QuickAccessProvider,
  type QuickInputItem,
  type QuickInputService as QuickInputServiceContract,
  type QuickInputShowOptions,
} from "../../services/quick-input-service";

/** Narrows a factory's unknown product to the provider shape. */
function asProvider(value: unknown): QuickAccessProvider | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }
  if (typeof (value as { getItems?: unknown }).getItems !== "function") {
    return undefined;
  }
  return value as QuickAccessProvider;
}

/** Registry overrides; tests inject their own instances. */
export interface QuickInputDependencies {
  readonly registry?: QuickAccessRegistry;
  /** Where the modes list looks up each command's keybinding. */
  readonly keybindings?: KeybindingsRegistry;
}

/** One rendered row: its element and its accept behavior. */
interface Row {
  readonly element: HTMLLIElement;
  readonly accept: () => void;
}

/** How many times one input value may be redirected before routing gives up. */
const MAX_REDIRECTS = 8;

let nextInstanceId = 0;

/** A text node, or the label's matched span wrapped for the highlight color. */
function appendLabel(into: HTMLElement, label: string, highlights: readonly LabelHighlight[] | undefined): void {
  let cursor = 0;
  for (const { start, end } of [...(highlights ?? [])].sort((a, b) => a.start - b.start)) {
    // Ignore a span that overlaps the last one or runs off the label.
    if (start < cursor || end > label.length || end <= start) {
      continue;
    }
    if (start > cursor) {
      into.append(label.slice(cursor, start));
    }
    const mark = document.createElement("span");
    mark.className = "ws-quick-input__highlight";
    mark.textContent = label.slice(start, end);
    into.append(mark);
    cursor = end;
  }
  if (cursor < label.length) {
    into.append(label.slice(cursor));
  }
}

/**
 * The key chips of a keybinding label such as "Ctrl+Shift+P" or
 * "Ctrl+K Ctrl+S": a chip per key with a "+" between the keys of one
 * chord and a gap between chords. The label's text is unchanged, so it
 * still reads as the label to a screen reader.
 */
function renderKeybinding(label: string): HTMLElement {
  const keybinding = document.createElement("span");
  keybinding.className = "ws-quick-input__option-keybinding";
  label
    .split(" ")
    .filter((chord) => chord !== "")
    .forEach((chord, chordIndex) => {
      if (chordIndex > 0) {
        const gap = document.createElement("span");
        gap.className = "ws-quick-input__chord-separator";
        keybinding.append(gap);
      }
      chord.split("+").forEach((key, keyIndex) => {
        if (keyIndex > 0) {
          const plus = document.createElement("span");
          plus.className = "ws-quick-input__key-separator";
          plus.textContent = "+";
          keybinding.append(plus);
        }
        const chip = document.createElement("span");
        chip.className = "ws-quick-input__key";
        chip.textContent = key;
        keybinding.append(chip);
      });
    });
  return keybinding;
}

/**
 * The quick input service. `quickAccess.show(value, options)` opens the
 * panel with `value` routed to its provider; the composition root
 * registers one instance under QUICK_INPUT_SERVICE and the quick-access
 * actions (showCommands, quickOpen, and friends) call it.
 */
export class QuickInputService extends Disposable implements QuickInputServiceContract {
  /** The quick-access surface the menu actions and command center call. */
  readonly quickAccess = {
    show: (value: string, options?: QuickInputShowOptions): void => {
      this.show(value, options);
    },
  };

  private readonly registry: QuickAccessRegistry;
  private readonly keybindings: KeybindingsRegistry;
  private readonly container: HTMLDivElement;
  private readonly input: HTMLInputElement;
  private readonly list: HTMLUListElement;
  private readonly listId: string;
  private rows: Row[] = [];
  private activeIndex = -1;
  private includeHelp = false;
  private isOpen = false;
  private restoreFocusTo: HTMLElement | null = null;
  private readonly onOutsidePointerDown = (event: Event): void => {
    const target = event.target;
    if (target instanceof Node && this.container.contains(target)) {
      return;
    }
    this.close(true);
  };

  constructor(deps: QuickInputDependencies = {}) {
    super();
    this.registry = deps.registry ?? QuickAccessRegistry;
    this.keybindings = deps.keybindings ?? KeybindingsRegistry;
    this.listId = `ws-quick-input-${++nextInstanceId}-list`;

    const label = document.createElement("label");
    label.className = "ws-quick-input__label";
    label.htmlFor = `${this.listId}-input`;
    label.textContent = "Search files, commands, and more";

    this.input = document.createElement("input");
    this.input.id = `${this.listId}-input`;
    this.input.className = "ws-quick-input__input";
    this.input.type = "text";
    this.input.setAttribute("role", "combobox");
    this.input.setAttribute("aria-autocomplete", "list");
    this.input.setAttribute("aria-expanded", "false");
    this.input.setAttribute("aria-controls", this.listId);
    this.input.addEventListener("input", () => this.route());
    this.input.addEventListener("keydown", (event) => this.onKeydown(event));

    // The box sits in a header of its own, with no divider under it.
    const header = document.createElement("div");
    header.className = "ws-quick-input__header";
    header.appendChild(this.input);

    this.list = document.createElement("ul");
    this.list.id = this.listId;
    this.list.className = "ws-quick-input__list";
    this.list.setAttribute("role", "listbox");

    this.container = document.createElement("div");
    this.container.className = "ws-quick-input";
    this.container.hidden = true;
    this.container.appendChild(label);
    this.container.appendChild(header);
    this.container.appendChild(this.list);
    document.body.appendChild(this.container);
    this._register(toDisposable(() => this.container.remove()));
  }

  /** Shows the panel with `value` routed to its provider. */
  show(value: string, options: QuickInputShowOptions = {}): void {
    if (!this.isOpen) {
      const active = document.activeElement;
      this.restoreFocusTo = active instanceof HTMLElement ? active : null;
      document.addEventListener("pointerdown", this.onOutsidePointerDown);
      this.isOpen = true;
    }
    this.includeHelp = options.includeHelp === true;
    this.input.value = value;
    this.container.hidden = false;
    this.input.setAttribute("aria-expanded", "true");
    this.route();
    this.input.focus();
  }

  /** Dismisses the panel. Safe when closed. */
  close(restoreFocus = false): void {
    if (!this.isOpen) {
      return;
    }
    this.isOpen = false;
    document.removeEventListener("pointerdown", this.onOutsidePointerDown);
    this.container.hidden = true;
    this.input.setAttribute("aria-expanded", "false");
    if (restoreFocus) {
      this.restoreFocusTo?.focus();
    }
    this.restoreFocusTo = null;
  }

  /**
   * Re-routes the current input value and rebuilds the rows. A provider
   * may send the input to another mode first; the value is rewritten and
   * routed again, a few hops at most.
   */
  private route(): void {
    let provider: QuickAccessProvider | undefined;
    let filter = "";
    for (let hops = 0; ; hops += 1) {
      const value = this.input.value;
      const descriptor = this.registry.getQuickAccessProvider(value);
      this.input.placeholder = descriptor?.placeholder ?? "";
      provider = descriptor === undefined ? undefined : asProvider(descriptor.factory());
      filter = descriptor === undefined ? value : value.slice(descriptor.prefix.length);
      const redirected = provider?.redirect?.(filter);
      if (redirected === undefined || redirected === value || hops >= MAX_REDIRECTS) {
        break;
      }
      this.input.value = redirected;
    }
    this.rebuildRows(provider?.getItems(filter) ?? [], provider?.noResultsMessage);
  }

  private rebuildRows(items: readonly QuickInputItem[], noResultsMessage: string | undefined): void {
    this.list.textContent = "";
    this.rows = [];
    const entries: QuickInputItem[] = [];
    if (this.includeHelp && this.input.value === "") {
      entries.push(...this.modeItems());
    }
    entries.push(
      ...items.map((item) => ({
        ...item,
        accept: () => {
          // Close first, so an accept that reopens quick input never has
          // its fresh panel dismissed by a stale close.
          this.close();
          item.accept();
        },
      })),
    );
    for (const entry of entries) {
      if (entry.separator !== undefined) {
        this.list.appendChild(this.buildSeparator(entry.separator));
      }
      const row = this.buildRow(entry);
      this.rows.push(row);
      this.list.appendChild(row.element);
    }
    if (entries.length === 0 && noResultsMessage !== undefined) {
      const empty = document.createElement("li");
      empty.className = "ws-quick-input__empty";
      empty.setAttribute("role", "presentation");
      empty.textContent = noResultsMessage;
      this.list.appendChild(empty);
    }
    this.setActiveIndex(this.rows.length === 0 ? -1 : 0);
  }

  /**
   * The modes list: the help entries with a commandCenterOrder, in that
   * order. A row's label is its commandCenterLabel or description, its
   * description the entry's prefix (the provider's own when it has none),
   * and its keybinding the command's. Accepting a row enters the mode in
   * place, never closing the panel.
   */
  private modeItems(): QuickInputItem[] {
    const modes = this.registry
      .getQuickAccessProviders()
      .flatMap((descriptor) =>
        descriptor.helpEntries
          .filter((entry) => entry.commandCenterOrder !== undefined)
          .map((entry) => ({ descriptor, entry, order: entry.commandCenterOrder ?? 0 })),
      )
      .sort((a, b) => a.order - b.order);
    return modes.map(({ descriptor, entry }) => ({
      label: entry.commandCenterLabel ?? entry.description,
      description: entry.prefix ?? descriptor.prefix,
      keybinding:
        entry.commandId === undefined ? undefined : this.keybindings.lookupKeybinding(entry.commandId)?.getLabel(),
      accept: () => {
        this.input.value = descriptor.prefix;
        this.route();
        this.input.focus();
      },
    }));
  }

  /** A group label: a presentational line above the row it opens. */
  private buildSeparator(text: string): HTMLLIElement {
    const element = document.createElement("li");
    element.className = "ws-quick-input__separator";
    element.setAttribute("role", "presentation");
    element.textContent = text;
    return element;
  }

  private buildRow(item: QuickInputItem): Row {
    const element = document.createElement("li");
    element.className = "ws-quick-input__option";
    element.id = `${this.listId}-option-${this.rows.length}`;
    element.setAttribute("role", "option");
    element.setAttribute("aria-selected", "false");
    const label = document.createElement("span");
    label.className = "ws-quick-input__option-label";
    appendLabel(label, item.label, item.labelHighlights);
    element.appendChild(label);
    if (item.description !== undefined && item.description !== "") {
      const description = document.createElement("span");
      description.className = "ws-quick-input__option-description";
      description.textContent = item.description;
      element.appendChild(description);
    }
    if (item.keybinding !== undefined && item.keybinding !== "") {
      element.appendChild(renderKeybinding(item.keybinding));
    }
    const row: Row = { element, accept: item.accept };
    element.addEventListener("pointerdown", (event) => {
      // Keep the input's focus; the click accepts the row.
      event.preventDefault();
    });
    element.addEventListener("click", () => row.accept());
    return row;
  }

  private setActiveIndex(index: number): void {
    this.activeIndex = index;
    for (const [rowIndex, row] of this.rows.entries()) {
      row.element.setAttribute("aria-selected", rowIndex === index ? "true" : "false");
    }
    const active = index === -1 ? undefined : this.rows[index];
    if (active === undefined) {
      this.input.removeAttribute("aria-activedescendant");
    } else {
      this.input.setAttribute("aria-activedescendant", active.element.id);
    }
  }

  private moveActive(delta: number): void {
    const count = this.rows.length;
    if (count === 0) {
      return;
    }
    this.setActiveIndex((this.activeIndex + delta + count) % count);
  }

  private onKeydown(event: KeyboardEvent): void {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        this.moveActive(1);
        break;
      case "ArrowUp":
        event.preventDefault();
        this.moveActive(-1);
        break;
      case "Enter": {
        event.preventDefault();
        const row = this.activeIndex === -1 ? undefined : this.rows[this.activeIndex];
        row?.accept();
        break;
      }
      case "Escape":
        event.preventDefault();
        this.close(true);
        break;
    }
  }
}
