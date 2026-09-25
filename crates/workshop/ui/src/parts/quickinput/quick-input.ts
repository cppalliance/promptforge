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
// provider is the input value minus the prefix.
//
// Providers are narrowed, never cast: the registry stores factories
// returning unknown, and a factory whose product lacks getItems renders
// an empty list rather than throwing at open.

import "./quick-input.css";

import { Disposable, toDisposable } from "../../base/lifecycle";
import { QuickAccessRegistry } from "../../services/quick-access-registry";
import { type QuickAccessProvider, type QuickInputItem, type QuickInputShowOptions } from "../../services/quick-input-service";

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

/** Registry override; tests inject their own instance. */
export interface QuickInputDependencies {
  readonly registry?: QuickAccessRegistry;
}

/** One rendered row: its element and its accept behavior. */
interface Row {
  readonly element: HTMLLIElement;
  readonly accept: () => void;
}

let nextInstanceId = 0;

/**
 * The quick input service. `quickAccess.show(value, options)` opens the
 * panel with `value` routed to its provider; the composition root
 * registers one instance under QUICK_INPUT_SERVICE and the quick-access
 * actions (showCommands, quickOpen, and friends) call it.
 */
export class QuickInputService extends Disposable {
  /** The quick-access surface the menu actions and command center call. */
  readonly quickAccess = {
    show: (value: string, options?: QuickInputShowOptions): void => {
      this.show(value, options);
    },
  };

  private readonly registry: QuickAccessRegistry;
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

    this.list = document.createElement("ul");
    this.list.id = this.listId;
    this.list.className = "ws-quick-input__list";
    this.list.setAttribute("role", "listbox");

    this.container = document.createElement("div");
    this.container.className = "ws-quick-input";
    this.container.hidden = true;
    this.container.appendChild(label);
    this.container.appendChild(this.input);
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

  /** Re-routes the current input value and rebuilds the rows. */
  private route(): void {
    const value = this.input.value;
    const descriptor = this.registry.getQuickAccessProvider(value);
    this.input.placeholder = descriptor?.placeholder ?? "";
    const provider = descriptor === undefined ? undefined : asProvider(descriptor.factory());
    const filter = descriptor === undefined ? value : value.slice(descriptor.prefix.length);
    this.rebuildRows(provider?.getItems(filter) ?? []);
  }

  private rebuildRows(items: readonly QuickInputItem[]): void {
    this.list.textContent = "";
    this.rows = [];
    if (this.includeHelp && this.input.value === "") {
      for (const descriptor of this.registry.getQuickAccessProviders()) {
        for (const entry of descriptor.helpEntries) {
          this.rows.push(
            this.buildRow(
              {
                label: entry.description,
                description: entry.prefix === "" ? undefined : entry.prefix,
                accept: () => {},
              },
              () => {
                // A help row enters its mode: re-route in place, never close.
                this.input.value = entry.prefix;
                this.route();
                this.input.focus();
              },
            ),
          );
        }
      }
    }
    for (const item of items) {
      this.rows.push(
        this.buildRow(item, () => {
          // Close first, so an accept that reopens quick input never has
          // its fresh panel dismissed by a stale close.
          this.close();
          item.accept();
        }),
      );
    }
    for (const row of this.rows) {
      this.list.appendChild(row.element);
    }
    this.setActiveIndex(this.rows.length === 0 ? -1 : 0);
  }

  private buildRow(item: QuickInputItem, accept: () => void): Row {
    const element = document.createElement("li");
    element.className = "ws-quick-input__option";
    element.id = `${this.listId}-option-${this.rows.length}`;
    element.setAttribute("role", "option");
    element.setAttribute("aria-selected", "false");
    const label = document.createElement("span");
    label.className = "ws-quick-input__option-label";
    label.textContent = item.label;
    element.appendChild(label);
    if (item.description !== undefined) {
      const description = document.createElement("span");
      description.className = "ws-quick-input__option-description";
      description.textContent = item.description;
      element.appendChild(description);
    }
    if (item.keybinding !== undefined) {
      const keybinding = document.createElement("span");
      keybinding.className = "ws-quick-input__option-keybinding";
      keybinding.textContent = item.keybinding;
      element.appendChild(keybinding);
    }
    const row: Row = { element, accept };
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

