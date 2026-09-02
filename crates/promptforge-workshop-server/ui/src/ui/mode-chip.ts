// The mode selector chip: a toolbar button showing the current agent
// mode's icon and label. Clicking opens a DropdownMenu of the four
// modes; picking one updates the chip and fires "agent-mode-changed" on
// document. UI-only by design - nothing here talks to the backend; the
// event is the seam the later wiring step consumes.

import "./mode-chip.css";

import {
  Bug,
  CircleQuestionMark,
  Infinity as InfinityIcon,
  ListTodo,
  createElement,
} from "lucide";
import type { IconNode } from "lucide";
import { Disposable, toDisposable } from "../base/lifecycle";
import { DropdownMenu } from "./workshop/dropdown";
import type { DropdownItem } from "./workshop/dropdown";

/** The agent interaction modes, keyed by display label. */
export const UNIFIED_MODES = {
  Agent: "agent",
  Ask: "ask",
  Plan: "plan",
  Debug: "debug",
} as const;

/** An agent interaction mode. */
export type UnifiedMode = (typeof UNIFIED_MODES)[keyof typeof UNIFIED_MODES];

/** The document-level event a mode selection fires; `detail` carries the mode. */
export const AGENT_MODE_CHANGED_EVENT = "agent-mode-changed";

// The labels in menu order, derived from UNIFIED_MODES so the dropdown
// can never drift from the mode set. Object.keys hides the literal key
// type; the cast restores it.
const MODE_LABELS = Object.keys(UNIFIED_MODES) as (keyof typeof UNIFIED_MODES)[];

/** Display labels by mode value: the inverse of UNIFIED_MODES. */
const MODE_LABEL_BY_MODE: Record<UnifiedMode, string> = {
  [UNIFIED_MODES.Agent]: "Agent",
  [UNIFIED_MODES.Ask]: "Ask",
  [UNIFIED_MODES.Plan]: "Plan",
  [UNIFIED_MODES.Debug]: "Debug",
};

const svg = (icon: IconNode, size: number): string =>
  createElement(icon, { width: size, height: size }).outerHTML;

/** Mode glyphs, rendered once at module load (the icons.ts pattern). */
const MODE_ICON_HTML: Record<UnifiedMode, string> = {
  [UNIFIED_MODES.Agent]: svg(InfinityIcon, 14),
  [UNIFIED_MODES.Ask]: svg(CircleQuestionMark, 14),
  [UNIFIED_MODES.Plan]: svg(ListTodo, 14),
  [UNIFIED_MODES.Debug]: svg(Bug, 14),
};

/**
 * The chip trigger plus its dropdown. Disposable: dispose() closes an
 * open menu and removes the trigger's click listener.
 */
export class ModeChip extends Disposable {
  /** The chip button; append it where the chip belongs. */
  readonly element: HTMLButtonElement;

  private readonly dropdown: DropdownMenu;
  private readonly iconSlot: HTMLSpanElement;
  private readonly labelSlot: HTMLSpanElement;
  private current: UnifiedMode = UNIFIED_MODES.Agent;

  constructor() {
    super();
    this.element = document.createElement("button");
    this.element.type = "button";
    this.element.className = "mode-chip";

    this.iconSlot = document.createElement("span");
    this.iconSlot.className = "mode-chip__icon";
    this.iconSlot.setAttribute("aria-hidden", "true");
    this.labelSlot = document.createElement("span");
    this.labelSlot.className = "mode-chip__label";
    this.element.append(this.iconSlot, this.labelSlot);

    this.dropdown = this._register(new DropdownMenu());

    const onClick = (): void => this.showMenu();
    this.element.addEventListener("click", onClick);
    this._register(
      toDisposable(() => this.element.removeEventListener("click", onClick)),
    );

    this.renderMode();
  }

  /** The selected mode. */
  get mode(): UnifiedMode {
    return this.current;
  }

  private showMenu(): void {
    const items: DropdownItem[] = MODE_LABELS.map((label) => {
      const mode = UNIFIED_MODES[label];
      return {
        label,
        iconHtml: MODE_ICON_HTML[mode],
        onClick: () => this.select(mode),
      };
    });
    this.dropdown.show(this.element, items);
  }

  private select(mode: UnifiedMode): void {
    // The event is "changed": re-picking the current mode stays silent.
    if (mode === this.current) {
      return;
    }
    this.current = mode;
    this.renderMode();
    document.dispatchEvent(
      new CustomEvent<UnifiedMode>(AGENT_MODE_CHANGED_EVENT, { detail: mode }),
    );
  }

  private renderMode(): void {
    // The icon HTML is this module's own static string, never input.
    this.iconSlot.innerHTML = MODE_ICON_HTML[this.current];
    this.labelSlot.textContent = MODE_LABEL_BY_MODE[this.current];
  }
}
