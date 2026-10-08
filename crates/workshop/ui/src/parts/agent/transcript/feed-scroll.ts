// The transcript feed's scroll behavior: it follows the bottom while the
// operator is at the bottom, lets go the moment the operator scrolls up,
// and picks up again on reaching the bottom.
//
// - Pinned: within 4px of the bottom. A feed starts pinned.
// - Release: scrolling up within 250ms of a wheel, touch, pointer, or key
//   input unpins. Scroll events with no recent input (our own follow, a
//   layout shift) never do.
// - Re-pin: reaching the bottom pins again.
// - Follow: while pinned, growth is detected by a ResizeObserver on the
//   content. If the new bottom is no more than one viewport away the feed
//   eases toward it at most 3.6px/ms and snaps there after 250ms; a bigger
//   jump, or reduced motion, goes straight there.
// - A send forces an instant pin.
//
// The trailing spacer under the last row is sized here too: 14px plus a
// fifth of the feed's height clamped to 80-240px, so the last message can
// scroll up out from under the composer.

import type { IDisposable } from "@workshop/platform/lifecycle";
import { prefersReducedMotion } from "./motion";

/** Within this many px of the bottom counts as at the bottom. */
export const PIN_THRESHOLD_PX = 4;

/** A scroll up this soon after an input event is the operator's. */
export const INPUT_WINDOW_MS = 250;

/** The follow's top speed. */
export const FOLLOW_SPEED_PX_PER_MS = 3.6;

/** The follow snaps to the bottom once it has run this long. */
export const FOLLOW_SNAP_MS = 250;

const SPACER_BASE_PX = 14;
const SPACER_MIN_PX = 80;
const SPACER_MAX_PX = 240;
const SPACER_SHARE = 0.2;

/** The trailing spacer's height for a feed `viewportHeight` tall. */
export function spacerHeight(viewportHeight: number): number {
  const share = Math.round(viewportHeight * SPACER_SHARE);
  return SPACER_BASE_PX + Math.min(SPACER_MAX_PX, Math.max(SPACER_MIN_PX, share));
}

/** The inputs that open the release window. */
const INPUT_EVENTS = ["wheel", "touchstart", "touchmove", "pointerdown", "keydown"] as const;

export interface FeedScrollOptions {
  /** The element under the content that takes the trailing spacer's height. */
  readonly spacer?: HTMLElement;
  /** A millisecond clock for the input window; defaults to `performance.now()`. */
  readonly now?: () => number;
  /** Called after the scroll position (`scroll`) or the feed's or content's size (`resize`) changed. */
  readonly onLayout?: (reason: "scroll" | "resize") => void;
}

/** The scroll controller for one feed. */
export class FeedScroll implements IDisposable {
  private isPinned = true;
  private lastInputAt = Number.NEGATIVE_INFINITY;
  private lastScrollTop = 0;
  private lastScrollHeight = 0;
  private lastClientHeight = -1;
  private frame: number | undefined;
  private followStartedAt: number | null = null;
  private followLastAt: number | null = null;
  private jumpNext = false;
  private readonly now: () => number;
  private readonly observer: ResizeObserver | undefined;

  private readonly onInput = (): void => {
    this.lastInputAt = this.now();
  };
  private readonly onScroll = (): void => {
    const top = this.scroller.scrollTop;
    if (this.distanceFromBottom() <= PIN_THRESHOLD_PX) {
      this.isPinned = true;
    } else if (top < this.lastScrollTop && this.now() - this.lastInputAt <= INPUT_WINDOW_MS) {
      this.isPinned = false;
      this.stopFollow();
    }
    this.lastScrollTop = top;
    this.options.onLayout?.("scroll");
  };

  constructor(
    private readonly scroller: HTMLElement,
    private readonly content: HTMLElement,
    private readonly options: FeedScrollOptions = {},
  ) {
    this.now = options.now ?? (() => performance.now());
    for (const type of INPUT_EVENTS) {
      scroller.addEventListener(type, this.onInput, { passive: true });
    }
    scroller.addEventListener("scroll", this.onScroll, { passive: true });
    this.lastScrollHeight = scroller.scrollHeight;
    this.lastScrollTop = scroller.scrollTop;
    if (typeof ResizeObserver !== "undefined") {
      this.observer = new ResizeObserver(() => this.onResize());
      this.observer.observe(content);
      this.observer.observe(scroller);
    }
    this.syncSpacer();
  }

  /** Whether the feed is following the bottom. */
  get pinned(): boolean {
    return this.isPinned;
  }

  /** Pins and jumps to the bottom at once: what a send does. */
  forcePin(): void {
    this.isPinned = true;
    this.jumpNext = true;
    this.stopFollow();
    this.jumpToBottom();
  }

  dispose(): void {
    for (const type of INPUT_EVENTS) {
      this.scroller.removeEventListener(type, this.onInput);
    }
    this.scroller.removeEventListener("scroll", this.onScroll);
    this.observer?.disconnect();
    this.stopFollow();
  }

  private distanceFromBottom(): number {
    const { scrollHeight, clientHeight, scrollTop } = this.scroller;
    return scrollHeight - clientHeight - scrollTop;
  }

  private maxScrollTop(): number {
    return Math.max(0, this.scroller.scrollHeight - this.scroller.clientHeight);
  }

  private jumpToBottom(): void {
    this.scroller.scrollTop = this.maxScrollTop();
    this.lastScrollTop = this.scroller.scrollTop;
  }

  private syncSpacer(): void {
    const spacer = this.options.spacer;
    const height = this.scroller.clientHeight;
    if (spacer !== undefined && height !== this.lastClientHeight) {
      this.lastClientHeight = height;
      spacer.style.height = `${spacerHeight(height)}px`;
    }
  }

  /** The content or the feed changed size. */
  private onResize(): void {
    this.syncSpacer();
    const height = this.scroller.scrollHeight;
    const grew = height > this.lastScrollHeight;
    this.lastScrollHeight = height;
    if (this.isPinned) {
      this.follow(grew);
    }
    this.options.onLayout?.("resize");
  }

  private follow(grew: boolean): void {
    const gap = this.maxScrollTop() - this.scroller.scrollTop;
    if (gap <= 0) {
      return;
    }
    if (this.jumpNext || !grew || prefersReducedMotion() || gap > this.scroller.clientHeight) {
      this.jumpNext = false;
      this.stopFollow();
      this.jumpToBottom();
      return;
    }
    if (this.frame === undefined) {
      this.followStartedAt = null;
      this.followLastAt = null;
      this.frame = requestAnimationFrame((time) => this.step(time));
    }
  }

  private step(time: number): void {
    this.frame = undefined;
    const startedAt = this.followStartedAt ?? time;
    const lastAt = this.followLastAt ?? time;
    this.followStartedAt = startedAt;
    this.followLastAt = time;

    const target = this.maxScrollTop();
    const remaining = target - this.scroller.scrollTop;
    if (remaining <= 0) {
      this.stopFollow();
      return;
    }
    if (time - startedAt >= FOLLOW_SNAP_MS) {
      this.stopFollow();
      this.jumpToBottom();
      return;
    }
    // The first frame has no elapsed time yet; take one 60Hz frame's worth.
    const elapsed = time === lastAt ? 16 : time - lastAt;
    const distance = Math.min(remaining, FOLLOW_SPEED_PX_PER_MS * elapsed);
    this.scroller.scrollTop += distance;
    this.lastScrollTop = this.scroller.scrollTop;
    if (remaining - distance > 0) {
      this.frame = requestAnimationFrame((next) => this.step(next));
    } else {
      this.stopFollow();
    }
  }

  private stopFollow(): void {
    if (this.frame !== undefined) {
      cancelAnimationFrame(this.frame);
      this.frame = undefined;
    }
    this.followStartedAt = null;
    this.followLastAt = null;
  }
}
