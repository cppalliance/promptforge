// The operator's message row: no label, full column width, a bordered
// card of plain text. A long message is clipped to three and a half lines
// with a fade over the last of them; clicking toggles it between clipped
// and expanded, and only a message that overflows reads as clickable.
// While its turn is on screen the card sticks to the top of the feed so
// the question stays in view over the answer (CSS `position: sticky`
// inside the turn wrapper); a message taller than the feed doesn't stick,
// and a fade below the stuck card shows only while it is stuck.
//
// The text is user-authored and lands through textContent, whitespace
// preserved by CSS. Expanded state lives here and survives `update`.

import "./human-message.css";

import type { IDisposable } from "@workshop/platform/lifecycle";
import type { HumanRow } from "./transcript-model";

/** The height the clipped text shows: three and a half 22px lines. Keep in step with human-message.css. */
export const HUMAN_CLIP_PX = 77;

/** The padding the clip box adds around its text (2px above, 8px below), which scrollHeight counts. */
const HUMAN_BLEED_PX = 10;

/** One human message row. */
export class HumanMessage implements IDisposable {
  readonly kind = "human";
  readonly element: HTMLDivElement;

  private readonly text: HTMLDivElement;
  private source = "";
  private expanded = false;
  private overflowing = false;
  private dirty = true;

  constructor() {
    this.element = document.createElement("div");
    this.element.className = "ws-human-message";
    this.text = document.createElement("div");
    this.text.className = "ws-human-message__text";
    this.element.appendChild(this.text);

    this.text.addEventListener("click", () => {
      // A drag-selection ends in a click; leave the card as it is then.
      if (this.overflowing && (window.getSelection()?.toString() ?? "") === "") {
        this.toggle();
      }
    });
    this.text.addEventListener("keydown", (event) => {
      if (this.overflowing && (event.key === "Enter" || event.key === " ")) {
        event.preventDefault();
        this.toggle();
      }
    });
    this.render();
  }

  /** Sets the message text; the expanded state is left alone. */
  update(row: HumanRow): void {
    if (row.text !== this.source) {
      this.source = row.text;
      this.text.textContent = row.text;
      this.dirty = true;
    }
  }

  /** The message text, for the transcript's Copy Message. */
  copyText(): string {
    return this.source;
  }

  /**
   * Re-reads whether the text overflows the clip. The view calls it once
   * the element is in the document and on every resize of the feed
   * (`force`), because a width change rewraps the text; an unchanged
   * message otherwise skips the layout read.
   */
  measure(force = false): void {
    if (!this.dirty && !force) {
      return;
    }
    this.dirty = false;
    const overflowing = this.text.scrollHeight - HUMAN_BLEED_PX > HUMAN_CLIP_PX;
    if (overflowing !== this.overflowing) {
      this.overflowing = overflowing;
      this.render();
    }
  }

  /**
   * Updates the sticky state against the feed: a message taller than the
   * feed doesn't stick, and one pinned at the feed's top edge is stuck
   * (which shows the fade below it).
   */
  layout(scroller: HTMLElement): void {
    const rect = this.element.getBoundingClientRect();
    const viewport = scroller.clientHeight;
    const tall = viewport > 0 && rect.height > viewport;
    const stuck =
      !tall && scroller.scrollTop > 0 && rect.top <= scroller.getBoundingClientRect().top + 1;
    this.element.dataset["tall"] = String(tall);
    this.element.dataset["stuck"] = String(stuck);
  }

  dispose(): void {
    // Element-owned listeners die with the element.
  }

  private toggle(): void {
    this.expanded = !this.expanded;
    this.render();
  }

  private render(): void {
    this.element.dataset["overflowing"] = String(this.overflowing);
    this.element.dataset["expanded"] = String(this.expanded);
    this.text.dataset["clipped"] = String(this.overflowing && !this.expanded);
    if (this.overflowing) {
      this.text.setAttribute("role", "button");
      this.text.tabIndex = 0;
      this.text.setAttribute("aria-expanded", String(this.expanded));
    } else {
      this.text.removeAttribute("role");
      this.text.removeAttribute("tabindex");
      this.text.removeAttribute("aria-expanded");
    }
  }
}
