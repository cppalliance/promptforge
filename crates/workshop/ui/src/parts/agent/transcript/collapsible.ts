// The transcript's shared collapsible: one header line (an action in the
// row's tone, details after it, a trailing chevron) over a body that
// animates its height open. Thought rows, group rows, a group's thinking
// lines, and generic tool lines all build on it.
//
// The open state lives here, not in the model: a row's `update(row)` calls
// the header setters again on every delta and settle, and none of them
// touches the state. The state has two layers - a default the owner sets
// (a streaming thinking line opens by default, a settled one doesn't) and
// the operator's own choice, which wins once made, so a click survives
// every later update. Default closed.
//
// The body is clipped to zero height while closed (CSS animates it) and
// marked `inert`, so its links and focusable children leave the tab order.
// Header text is untrusted model-era data and always lands through
// textContent.

import "./collapsible.css";

import { ICON_CHEVRON_RIGHT } from "@workshop/look/icons";
import { setShimmer } from "@workshop/look/shimmer";

/** The shimmer tones a header's action can run in (shimmer.css). */
export type ShimmerTone = "status" | "thinking" | "tool";

const TONES: readonly ShimmerTone[] = ["status", "thinking", "tool"];

/** Runs, or stops, the shared shimmer on `element` in the given tone. */
export function applyShimmer(element: HTMLElement, tone: ShimmerTone | null): void {
  for (const candidate of TONES) {
    element.classList.toggle(`ws-shimmer-text--${candidate}`, candidate === tone);
  }
  setShimmer(element, tone !== null);
}

/** What a header reads. */
export interface CollapsibleHeader {
  /** The verb or title, in the row's tone. */
  readonly action: string;
  /** The text after it, or null for none. */
  readonly details: string | null;
  /** The call name leading `details`, rendered in the verb's color; null when `details` is plain. */
  readonly callName?: string | null;
  /** The tone the action shimmers in, or null for a still action. */
  readonly shimmer?: ShimmerTone | null;
}

export interface CollapsibleOptions {
  /** Called with the effective open state each time it changes. */
  readonly onToggle?: (open: boolean) => void;
}

/** One collapsible: header, chevron, and an animated body. */
export class Collapsible {
  /** The root; append it where the row belongs. */
  readonly element: HTMLDivElement;
  /** The header line. */
  readonly header: HTMLDivElement;
  /** The body the owner fills. */
  readonly body: HTMLDivElement;

  private readonly action: HTMLSpanElement;
  private readonly details: HTMLSpanElement;
  private readonly clip: HTMLDivElement;
  private readonly onToggle: ((open: boolean) => void) | undefined;
  private detailsKey: string | null = null;
  private expandable = true;
  private defaultOpen = false;
  private choice: boolean | null = null;
  private shown = false;
  private state = "";

  constructor(options: CollapsibleOptions = {}) {
    this.onToggle = options.onToggle;

    this.element = document.createElement("div");
    this.element.className = "ws-collapsible";

    this.header = document.createElement("div");
    this.header.className = "ws-collapsible__header";
    this.action = document.createElement("span");
    this.action.className = "ws-collapsible__action";
    this.details = document.createElement("span");
    this.details.className = "ws-collapsible__details";
    const chevron = document.createElement("span");
    chevron.className = "ws-collapsible__chevron";
    chevron.setAttribute("aria-hidden", "true");
    // A static string from @workshop/look, never data.
    chevron.innerHTML = ICON_CHEVRON_RIGHT;
    this.header.append(this.action, this.details, chevron);

    this.clip = document.createElement("div");
    this.clip.className = "ws-collapsible__clip";
    this.body = document.createElement("div");
    this.body.className = "ws-collapsible__body";
    this.clip.appendChild(this.body);
    this.element.append(this.header, this.clip);

    this.header.addEventListener("click", () => this.toggle());
    this.header.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        this.toggle();
      }
    });
    this.refresh();
  }

  /** The effective open state: the operator's choice when made, else the default. */
  get open(): boolean {
    return this.expandable && (this.choice ?? this.defaultOpen);
  }

  /** The operator's own choice (true open, false closed), or null while they've made none. */
  get operatorChoice(): boolean | null {
    return this.choice;
  }

  /**
   * Takes over the operator's choice from the component this one replaces,
   * so a row that turns into another kind under the same key (a thought
   * that becomes a group) keeps what the operator opened or closed.
   */
  adoptChoice(choice: boolean | null): void {
    this.choice = choice;
    this.refresh();
  }

  /**
   * Sets what the open state is until the operator chooses: a streaming
   * thinking line opens by default and closes by default once settled. A
   * choice the operator already made is left alone.
   */
  setDefaultOpen(open: boolean): void {
    this.defaultOpen = open;
    this.refresh();
  }

  /**
   * Whether the line has anything to open. A line without a body shows no
   * chevron, takes no clicks, and is not a button.
   */
  setExpandable(expandable: boolean): void {
    this.expandable = expandable;
    this.refresh();
  }

  /** Paints the header, touching the DOM only where the text changed. */
  setHeader(header: CollapsibleHeader): void {
    if (this.action.textContent !== header.action) {
      this.action.textContent = header.action;
    }
    const callName = header.callName ?? null;
    const details = header.details ?? "";
    const key = `${callName ?? ""}\u0000${details}`;
    if (key !== this.detailsKey) {
      this.detailsKey = key;
      this.details.replaceChildren();
      if (callName !== null && callName !== "" && details.startsWith(callName)) {
        const call = document.createElement("span");
        call.className = "ws-collapsible__call";
        call.textContent = callName;
        this.details.append(call, details.slice(callName.length));
      } else {
        this.details.textContent = details;
      }
      this.details.hidden = details === "";
    }
    applyShimmer(this.action, header.shimmer ?? null);
  }

  private toggle(): void {
    if (!this.expandable) {
      return;
    }
    this.choice = !this.open;
    this.refresh();
  }

  private refresh(): void {
    const open = this.open;
    // An update that changes neither flag writes nothing to the DOM.
    const state = `${open}|${this.expandable}`;
    if (state === this.state) {
      return;
    }
    this.state = state;
    this.element.dataset["open"] = String(open);
    this.element.dataset["expandable"] = String(this.expandable);
    this.clip.toggleAttribute("inert", !open);
    this.clip.setAttribute("aria-hidden", String(!open));
    if (this.expandable) {
      this.header.setAttribute("role", "button");
      this.header.tabIndex = 0;
      this.header.setAttribute("aria-expanded", String(open));
    } else {
      this.header.removeAttribute("role");
      this.header.removeAttribute("tabindex");
      this.header.removeAttribute("aria-expanded");
    }
    if (open !== this.shown) {
      this.shown = open;
      this.onToggle?.(open);
    }
  }
}
