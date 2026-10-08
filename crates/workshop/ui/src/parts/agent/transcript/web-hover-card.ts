// The hover card a web search or fetch line opens: the result titles and
// URLs behind "Searched web" / "Fetched page". It opens 600ms after the
// pointer settles on the line, never while the call is still loading,
// sits below the line's start edge, and closes when the pointer leaves
// both, on Escape, or when a link is clicked.
//
// Everything in a card is untrusted tool output or model-authored
// arguments. Text lands through textContent, and only `http:` and `https:`
// URLs become anchors - built like the markdown renderer's `link` method
// (escaped by the DOM, not draggable, the URL as its own title) - so a
// `javascript:` or `file:` URL shows as plain text and is never clickable.

import "./tool-line.css";

import type { IDisposable } from "@workshop/platform/lifecycle";

/** One line of a card: a title over its URL. */
export interface WebCardEntry {
  readonly title: string;
  readonly url: string;
}

/** The pointer must rest this long on a line before its card opens. */
export const WEB_CARD_OPEN_DELAY_MS = 600;

/** Leaving a line gives the pointer this long to reach the card before it closes. */
const CLOSE_GRACE_MS = 120;

/** The gap between a line and its card. */
const CARD_GAP_PX = 4;

/** The space kept clear of the viewport edge. */
const EDGE_PX = 8;

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/**
 * The entries a card lists, or null when there is no card. A search reads
 * its result, `{query, results: [{title, url, description}]}` - one entry
 * per result that carries a URL - and a result that doesn't parse as that
 * shape (or lists nothing) gives no card. A fetch lists the URL it was
 * asked for.
 */
export function webCardEntries(
  kind: "search" | "fetch",
  args: string,
  result: string | null,
): WebCardEntry[] | null {
  if (kind === "fetch") {
    const url = asRecord(parseJson(args))?.["url"];
    return typeof url === "string" && url !== "" ? [{ title: url, url }] : null;
  }
  if (result === null) {
    return null;
  }
  const results = asRecord(parseJson(result))?.["results"];
  if (!Array.isArray(results)) {
    return null;
  }
  const entries: WebCardEntry[] = [];
  for (const candidate of results) {
    const record = asRecord(candidate);
    const url = record?.["url"];
    if (record === null || typeof url !== "string" || url === "") {
      continue;
    }
    const title = record["title"];
    entries.push({ title: typeof title === "string" && title !== "" ? title : url, url });
  }
  return entries.length > 0 ? entries : null;
}

/** The URL as an anchor target when it is http(s), else null. */
function webHref(raw: string): string | null {
  try {
    const parsed = new URL(raw);
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed.href : null;
  } catch {
    return null;
  }
}

/** One card, attached to the line that opens it. */
export class WebHoverCard implements IDisposable {
  private entries: readonly WebCardEntry[] | null = null;
  private disabled = false;
  private card: HTMLDivElement | null = null;
  private openTimer: ReturnType<typeof setTimeout> | undefined;
  private closeTimer: ReturnType<typeof setTimeout> | undefined;

  private readonly onEnter = (): void => {
    this.clearTimers();
    if (this.card === null && this.canOpen()) {
      this.openTimer = setTimeout(() => this.open(), WEB_CARD_OPEN_DELAY_MS);
    }
  };
  private readonly onLeave = (): void => {
    clearTimeout(this.openTimer);
    this.scheduleClose();
  };
  private readonly onKeydown = (event: KeyboardEvent): void => {
    if (event.key === "Escape") {
      this.close();
    }
  };

  constructor(private readonly anchor: HTMLElement) {
    anchor.addEventListener("mouseenter", this.onEnter);
    anchor.addEventListener("mouseleave", this.onLeave);
  }

  /** Sets what the card lists; null (or an empty list) means no card. */
  setEntries(entries: readonly WebCardEntry[] | null): void {
    this.entries = entries !== null && entries.length > 0 ? entries : null;
    if (this.entries === null) {
      this.close();
    }
  }

  /** Disables the card while the call is loading. */
  setDisabled(disabled: boolean): void {
    this.disabled = disabled;
    if (disabled) {
      this.close();
    }
  }

  /** Closes the card and cancels a pending open. */
  close(): void {
    this.clearTimers();
    if (this.card === null) {
      return;
    }
    this.card.remove();
    this.card = null;
    document.removeEventListener("keydown", this.onKeydown);
  }

  dispose(): void {
    this.close();
    this.anchor.removeEventListener("mouseenter", this.onEnter);
    this.anchor.removeEventListener("mouseleave", this.onLeave);
  }

  private canOpen(): boolean {
    return !this.disabled && this.entries !== null;
  }

  private clearTimers(): void {
    clearTimeout(this.openTimer);
    clearTimeout(this.closeTimer);
  }

  private scheduleClose(): void {
    clearTimeout(this.closeTimer);
    if (this.card !== null) {
      this.closeTimer = setTimeout(() => this.close(), CLOSE_GRACE_MS);
    }
  }

  private open(): void {
    if (this.card !== null || !this.canOpen() || this.entries === null) {
      return;
    }
    const card = document.createElement("div");
    card.className = "ws-web-card";
    card.setAttribute("role", "group");
    card.setAttribute("aria-label", "Web results");
    for (const entry of this.entries) {
      card.appendChild(this.renderEntry(entry));
    }
    card.addEventListener("mouseenter", () => clearTimeout(this.closeTimer));
    card.addEventListener("mouseleave", () => this.scheduleClose());
    document.body.appendChild(card);
    this.card = card;
    document.addEventListener("keydown", this.onKeydown);
    this.place(card);
  }

  private renderEntry(entry: WebCardEntry): HTMLElement {
    const row = document.createElement("div");
    row.className = "ws-web-card__entry";
    const href = webHref(entry.url);
    let title: HTMLElement;
    if (href === null) {
      title = document.createElement("span");
    } else {
      const link = document.createElement("a");
      link.href = href;
      link.title = entry.url;
      link.draggable = false;
      // Opening a link ends the card: the system browser takes the click.
      link.addEventListener("click", () => this.close());
      title = link;
    }
    title.className = "ws-web-card__title";
    title.textContent = entry.title;
    const url = document.createElement("span");
    url.className = "ws-web-card__url";
    url.textContent = entry.url;
    row.append(title, url);
    return row;
  }

  /** Places the card below the line's start edge, flipped or nudged to stay on screen. */
  private place(card: HTMLElement): void {
    const anchor = this.anchor.getBoundingClientRect();
    const width = card.offsetWidth;
    const height = card.offsetHeight;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    let left = anchor.left;
    if (width > 0 && left + width > viewportWidth - EDGE_PX) {
      left = Math.max(EDGE_PX, viewportWidth - width - EDGE_PX);
    }
    let top = anchor.bottom + CARD_GAP_PX;
    if (height > 0 && top + height > viewportHeight - EDGE_PX && anchor.top - CARD_GAP_PX - height >= EDGE_PX) {
      top = anchor.top - CARD_GAP_PX - height;
    }
    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  }
}
