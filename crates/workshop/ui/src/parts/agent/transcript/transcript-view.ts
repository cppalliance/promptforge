// The transcript view: paints the model's turns, rows, and tail status
// into the scrolling feed. Every level reconciles by key (reconcile.ts):
// a turn's wrapper, a turn's rows, a group's steps. A key's component is
// reused and told `update(row)`, new keys get new components, gone keys
// lose theirs, and the tail goes last - or inside the open group that is
// the active tail - so a streaming delta changes text in place and never
// rebuilds a settled row. Open and expanded state lives in the row
// components and survives every delta and the settle.
//
// The view owns the feed's chrome: the centered column, the trailing
// spacer, the scroll behavior (feed-scroll.ts), the sticky human
// messages, each finished turn's Copy footer, and the transcript context
// menu (Copy Message, or Copy over a selection; Select All; Search with
// Google). Nothing here reads the service; the owner hands in the model.
//
// Every string painted here is untrusted data (user text, model markdown,
// tool arguments and output) and reaches the DOM through the row
// components: textContent, or renderMarkdown's DOMPurify pass.

import "./transcript.css";

import { DropdownMenu, type DropdownItem } from "@workshop/look/dropdown";
import type { IDisposable } from "@workshop/platform/lifecycle";
import { copyToClipboard } from "../../shared/clipboard";
import { FeedScroll } from "./feed-scroll";
import { HumanMessage } from "./human-message";
import { reconcile, type KeyedComponent } from "./reconcile";
import {
  carryRowState,
  createRowComponent,
  updateRowComponent,
  GroupRowView,
  type RowComponent,
} from "./rows";
import { TailView } from "./tail-status";
import { TurnFooter } from "./turn-footer";
import type { TranscriptModel, TranscriptTurn } from "./transcript-model";

/** What the view asks of its owner. */
export interface TranscriptHost {
  /** A turn's Copy landed: raise the confirmation. */
  copied(): void;
}

/** One turn: its wrapper, its rows, and its footer. */
class TurnView implements KeyedComponent {
  readonly kind = "turn";
  readonly element: HTMLDivElement;
  readonly rows = new Map<string, RowComponent & KeyedComponent>();

  private footer: TurnFooter | null = null;

  constructor(
    private readonly host: TranscriptHost,
    private readonly createRow: (row: TranscriptTurn["rows"][number]) => RowComponent & KeyedComponent,
  ) {
    this.element = document.createElement("div");
    this.element.className = "ws-turn";
  }

  /** Reconciles the turn's rows and its footer; `finished` shows the Copy footer. */
  update(turn: TranscriptTurn, finished: boolean): void {
    reconcile(
      this.element,
      this.rows,
      turn.rows,
      (row) => this.createRow(row),
      (component, row) => updateRowComponent(component, row),
      (component, replaced, row) => carryRowState(component, replaced, row.key),
    );
    const replies = turn.rows.flatMap((row) => (row.kind === "markdown" ? [row.text] : []));
    if (finished && replies.length > 0) {
      if (this.footer === null) {
        this.footer = new TurnFooter(() => this.host.copied());
      }
      this.footer.update(replies);
      if (this.element.lastElementChild !== this.footer.element) {
        this.element.appendChild(this.footer.element);
      }
    } else if (this.footer !== null) {
      this.footer.element.remove();
      this.footer.dispose();
      this.footer = null;
    }
  }

  dispose(): void {
    for (const row of this.rows.values()) {
      row.dispose();
    }
    this.rows.clear();
    this.footer?.dispose();
  }
}

/** The scrolling transcript. */
export class TranscriptView implements IDisposable {
  /** The scroller: the feed element the owner places in the session. */
  readonly element: HTMLDivElement;

  private readonly column: HTMLDivElement;
  private readonly turns = new Map<string, TurnView>();
  private readonly rowByElement = new WeakMap<Element, RowComponent>();
  private readonly tail: TailView;
  private readonly scroll: FeedScroll;
  private readonly menu = new DropdownMenu();
  private animateIn = false;

  private readonly onContextMenu = (event: MouseEvent): void => {
    this.openContextMenu(event);
  };

  constructor(private readonly host: TranscriptHost) {
    this.element = document.createElement("div");
    this.element.className = "ws-agent-session__feed";
    // A live region: the rows appended while the agent works are announced.
    this.element.setAttribute("role", "log");
    this.element.setAttribute("aria-live", "polite");
    this.element.setAttribute("aria-relevant", "additions");
    this.element.setAttribute("aria-atomic", "false");

    this.column = document.createElement("div");
    this.column.className = "ws-transcript";
    const spacer = document.createElement("div");
    spacer.className = "ws-transcript__spacer";
    spacer.setAttribute("aria-hidden", "true");
    this.column.appendChild(spacer);
    this.element.appendChild(this.column);

    this.tail = new TailView();
    this.scroll = new FeedScroll(this.element, this.column, {
      spacer,
      onLayout: (reason) => this.layoutHumans(reason === "resize"),
    });
    this.element.addEventListener("contextmenu", this.onContextMenu);
  }

  /** Pins to the bottom at once: what a send does. */
  forcePin(): void {
    this.scroll.forcePin();
  }

  /**
   * Paints the model. `generating` decides which turns are finished: every
   * turn but the last, and the last once the agent stops.
   */
  render(model: TranscriptModel, generating: boolean): void {
    const lastIndex = model.turns.length - 1;
    reconcile(
      this.column,
      this.turns,
      model.turns.map((turn) => ({ key: turn.key, kind: "turn", turn })),
      () => new TurnView(this.host, (row) => this.makeRow(row)),
      (view, entry) => {
        const finished = entry.turn !== model.turns[lastIndex] || !generating;
        view.update(entry.turn, finished);
      },
    );
    this.placeTail(model);
    // Rows that exist at the first paint appear in place; later ones fade in.
    this.animateIn = this.animateIn || model.turns.length > 0;
    for (const human of this.humans()) {
      human.measure();
    }
    this.layoutHumans(false);
  }

  dispose(): void {
    this.element.removeEventListener("contextmenu", this.onContextMenu);
    this.menu.dispose();
    this.scroll.dispose();
    this.tail.dispose();
    for (const turn of this.turns.values()) {
      turn.dispose();
    }
    this.turns.clear();
  }

  private makeRow(row: TranscriptTurn["rows"][number]): RowComponent & KeyedComponent {
    const component = createRowComponent(row, this.animateIn);
    this.rowByElement.set(component.element, component);
    return component;
  }

  /** The tail goes last in the last turn, or inside the group that is the active tail. */
  private placeTail(model: TranscriptModel): void {
    const status = model.tail;
    if (status === null) {
      this.tail.element.remove();
      return;
    }
    this.tail.update(status);
    const lastTurn = this.turns.get(model.turns[model.turns.length - 1]?.key ?? "");
    if (lastTurn === undefined) {
      this.tail.element.remove();
      return;
    }
    let host: HTMLElement = lastTurn.element;
    if (status.inGroup) {
      const lastRow = model.turns[model.turns.length - 1]?.rows.at(-1);
      const group = lastRow === undefined ? undefined : lastTurn.rows.get(lastRow.key);
      if (group instanceof GroupRowView) {
        host = group.element;
      }
    }
    if (this.tail.element.parentElement !== host || host.lastElementChild !== this.tail.element) {
      host.appendChild(this.tail.element);
    }
  }

  private humans(): HumanMessage[] {
    const found: HumanMessage[] = [];
    for (const turn of this.turns.values()) {
      for (const row of turn.rows.values()) {
        if (row instanceof HumanMessage) {
          found.push(row);
        }
      }
    }
    return found;
  }

  private layoutHumans(remeasure: boolean): void {
    for (const human of this.humans()) {
      if (remeasure) {
        human.measure(true);
      }
      human.layout(this.element);
    }
  }

  // --- Context menu -----------------------------------------------------------------

  private openContextMenu(event: MouseEvent): void {
    event.preventDefault();
    const selected = window.getSelection()?.toString() ?? "";
    const target = event.target instanceof Element ? event.target : null;
    const rowElement = target?.closest(".ws-transcript-row") ?? null;
    const component = rowElement === null ? undefined : this.rowByElement.get(rowElement);
    const message = component?.copyText() ?? "";
    const subject = selected !== "" ? selected : message;
    const items: DropdownItem[] = [];
    // A right-click outside any row (the tail, a turn footer, the spacer,
    // the column's padding) has no message and no selection to copy;
    // offering Copy there would overwrite the clipboard with nothing.
    if (subject !== "") {
      items.push({
        label: selected !== "" ? "Copy" : "Copy Message",
        onClick: () => {
          void copyToClipboard(subject);
        },
      });
    }
    items.push({
      label: "Select All",
      onClick: () => {
        window.getSelection()?.selectAllChildren(this.column);
      },
    });
    // Closed first so a second right-click on the feed opens a new menu
    // instead of toggling the open one shut.
    this.menu.close();
    this.menu.show(this.element, items, { x: event.clientX, y: event.clientY });
  }
}
