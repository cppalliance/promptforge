// The editor's find widget: a floating panel at the top right of the
// editor, in place of CodeMirror's stock search panel, built for
// search({ createPanel }). It copies Cursor's widget - a find row with
// the three option toggles, the match count, previous and next, and
// close, and a replace row behind a toggle - with Cursor's labels. It
// drives the stock search state (SearchQuery, findNext, replaceAll, and
// the rest), so the keymap, the Edit menu's Find and Replace rows, and
// the match highlighting all keep working. The look (419px wide, 33px
// rows, 28px from the right) lives in editor-panel.css.
//
// Glyphs: close and the replace toggle are codicons from @workshop/look;
// the rest of the buttons carry plain text, because the look package
// ships only the codicons the surface specifications use.

import { EditorView, runScopeHandlers, type Panel, type ViewUpdate } from "@codemirror/view";
import {
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  replaceAll,
  replaceNext,
  SearchQuery,
  setSearchQuery,
} from "@codemirror/search";

import { ICON_CHEVRON_RIGHT, ICON_CLOSE } from "@workshop/look/icons";

/** The widget's root class, which the stylesheet and revealReplace key on. */
const WIDGET_CLASS = "ws-find-widget";

/** Matches counted before the count reads "1000+" instead of a total. */
const COUNT_LIMIT = 1000;

/** The option toggles, as the query's own flag names. */
type Flag = "caseSensitive" | "wholeWord" | "regexp";

interface FlagToggle {
  readonly flag: Flag;
  readonly label: string;
  readonly glyph: string;
  /** The Alt+letter that flips it from either field. */
  readonly key: string;
}

const FLAG_TOGGLES: readonly FlagToggle[] = [
  { flag: "caseSensitive", label: "Match Case (Alt+C)", glyph: "Aa", key: "c" },
  { flag: "wholeWord", label: "Match Whole Word (Alt+W)", glyph: "ab", key: "w" },
  { flag: "regexp", label: "Use Regular Expression (Alt+R)", glyph: ".*", key: "r" },
];

/** Builds an element with a class. */
function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  return node;
}

/** A text field with Cursor's placeholder doubling as its accessible name. */
function textField(name: string, label: string): HTMLInputElement {
  const input = element("input", "ws-find-input");
  input.type = "text";
  input.name = name;
  input.placeholder = label;
  input.setAttribute("aria-label", label);
  input.spellcheck = false;
  input.autocomplete = "off";
  return input;
}

/** A button whose tooltip and accessible name are one label. */
function labelledButton(className: string, label: string): HTMLButtonElement {
  const button = element("button", className);
  button.type = "button";
  button.title = label;
  button.setAttribute("aria-label", label);
  return button;
}

class FindWidget implements Panel {
  readonly dom: HTMLElement;
  readonly top = true;

  private readonly searchField = textField("search", "Find");
  private readonly replaceField = textField("replace", "Replace");
  private readonly count = element("span", "ws-find-count");
  private readonly toggles = new Map<Flag, HTMLButtonElement>();
  private query: SearchQuery;

  constructor(private readonly view: EditorView) {
    this.query = getSearchQuery(view.state);
    this.searchField.setAttribute("main-field", "true");

    const dom = element("div", WIDGET_CLASS);
    dom.dataset.replace = "false";
    this.dom = dom;

    const reveal = labelledButton("ws-find-button ws-find-toggle", "Toggle Replace");
    reveal.innerHTML = ICON_CHEVRON_RIGHT;
    reveal.addEventListener("click", () => {
      dom.dataset.replace = dom.dataset.replace === "true" ? "false" : "true";
    });

    const options = element("div", "ws-find-options");
    for (const toggle of FLAG_TOGGLES) {
      const button = labelledButton("ws-find-button ws-find-option", toggle.label);
      button.textContent = toggle.glyph;
      button.addEventListener("click", () => this.flip(toggle.flag));
      this.toggles.set(toggle.flag, button);
      options.append(button);
    }
    const findRow = element("div", "ws-find-row");
    const findBox = element("div", "ws-find-field");
    findBox.append(this.searchField, options);
    this.count.setAttribute("aria-live", "polite");
    findRow.append(
      findBox,
      this.count,
      this.action("Previous Match (Shift+Enter)", "\u2191", () => findPrevious(view)),
      this.action("Next Match (Enter)", "\u2193", () => findNext(view)),
      this.action("Close (Escape)", "", () => closeSearchPanel(view), ICON_CLOSE),
    );

    const replaceRow = element("div", "ws-find-row ws-find-row--replace");
    const replaceBox = element("div", "ws-find-field");
    replaceBox.append(this.replaceField);
    replaceRow.append(
      replaceBox,
      this.action("Replace (Enter)", "Replace", () => replaceNext(view)),
      this.action("Replace All (Ctrl+Alt+Enter)", "All", () => replaceAll(view)),
    );

    const rows = element("div", "ws-find-rows");
    rows.append(findRow, replaceRow);
    dom.append(reveal, rows);

    dom.addEventListener("keydown", (event) => this.onKeydown(event));
    for (const field of [this.searchField, this.replaceField]) {
      field.addEventListener("input", () => this.commit());
      field.addEventListener("change", () => this.commit());
    }
    this.sync(this.query);
    this.refreshCount();
  }

  mount(): void {
    this.searchField.select();
  }

  update(update: ViewUpdate): void {
    let queryChanged = false;
    for (const transaction of update.transactions) {
      for (const effect of transaction.effects) {
        if (effect.is(setSearchQuery)) {
          queryChanged = true;
          if (!effect.value.eq(this.query)) {
            this.sync(effect.value);
          }
        }
      }
    }
    // The count scans the document, and this runs for every view update
    // (scroll, focus, geometry), so recount only when the answer can change:
    // a new query, an edit, or a moved selection.
    if (queryChanged || update.docChanged || update.selectionSet) {
      this.refreshCount();
    }
  }

  /** A button that runs `run`, with its label as tooltip and accessible name. */
  private action(label: string, glyph: string, run: () => void, icon?: string): HTMLButtonElement {
    const button = labelledButton("ws-find-button", label);
    if (icon === undefined) {
      button.textContent = glyph;
    } else {
      button.innerHTML = icon;
    }
    button.addEventListener("click", run);
    return button;
  }

  /** Shows `query` in the fields and the option toggles. */
  private sync(query: SearchQuery): void {
    this.query = query;
    this.searchField.value = query.search;
    this.replaceField.value = query.replace;
    for (const toggle of FLAG_TOGGLES) {
      this.toggles.get(toggle.flag)?.setAttribute("aria-pressed", String(query[toggle.flag]));
    }
  }

  /** Reads the fields and toggles back into the search state. */
  private commit(): void {
    const flag = (name: Flag): boolean => this.toggles.get(name)?.getAttribute("aria-pressed") === "true";
    const query = new SearchQuery({
      search: this.searchField.value,
      replace: this.replaceField.value,
      caseSensitive: flag("caseSensitive"),
      wholeWord: flag("wholeWord"),
      regexp: flag("regexp"),
      literal: this.query.literal,
    });
    if (!query.eq(this.query)) {
      this.query = query;
      this.view.dispatch({ effects: setSearchQuery.of(query) });
    }
  }

  private flip(flag: Flag): void {
    const button = this.toggles.get(flag);
    button?.setAttribute("aria-pressed", String(button.getAttribute("aria-pressed") !== "true"));
    this.commit();
  }

  /** "No results", or the current match's place among them ("?" while the selection is not on one). */
  private refreshCount(): void {
    const query = getSearchQuery(this.view.state);
    let total = 0;
    let current = 0;
    if (query.valid) {
      const { from, to } = this.view.state.selection.main;
      const cursor = query.getCursor(this.view.state);
      for (let match = cursor.next(); !match.done; match = cursor.next()) {
        total += 1;
        if (match.value.from === from && match.value.to === to) {
          current = total;
        }
        if (total >= COUNT_LIMIT) {
          break;
        }
      }
    }
    this.count.textContent =
      total === 0 ? "No results" : `${current === 0 ? "?" : current} of ${total >= COUNT_LIMIT ? `${COUNT_LIMIT}+` : total}`;
  }

  private onKeydown(event: KeyboardEvent): void {
    if (runScopeHandlers(this.view, event, "search-panel")) {
      event.preventDefault();
      return;
    }
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && event.altKey) {
      event.preventDefault();
      replaceAll(this.view);
      return;
    }
    if (event.key === "Enter" && event.target === this.searchField) {
      event.preventDefault();
      (event.shiftKey ? findPrevious : findNext)(this.view);
      return;
    }
    if (event.key === "Enter" && event.target === this.replaceField) {
      event.preventDefault();
      replaceNext(this.view);
      return;
    }
    if (event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
      const toggle = FLAG_TOGGLES.find((candidate) => candidate.key === event.key.toLowerCase());
      if (toggle !== undefined) {
        event.preventDefault();
        this.flip(toggle.flag);
      }
    }
  }
}

/** The search({ createPanel }) factory. */
export function createFindWidget(view: EditorView): Panel {
  return new FindWidget(view);
}

/**
 * Shows the widget's replace row, for Replace (Ctrl+H). A no-op when the
 * editor has no find widget open.
 */
export function revealReplace(view: EditorView): void {
  const widget = view.dom.querySelector<HTMLElement>(`.${WIDGET_CLASS}`);
  if (widget !== null) {
    widget.dataset.replace = "true";
  }
}
