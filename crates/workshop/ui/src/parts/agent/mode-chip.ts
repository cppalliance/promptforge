// The mode selector chip: a toolbar button showing the current agent
// mode's icon and label. Clicking it (or Ctrl+. and Shift+Tab, through
// openOrCycle) opens a DropdownMenu of the five modes on the composer's
// menu surface: check-only selection, a description on each row, opening
// upward at the chip's left edge minus 6px. Picking one updates the chip
// and fires "agent-mode-changed" on document and onDidChangeMode.
// UI-only by design - nothing here talks to the backend; the event is the
// seam for wiring the mode to the backend, and the session view listens
// to onDidChangeMode to color its action button.

import "./mode-chip.css";

import {
  ICON_AGENT,
  ICON_ASK,
  ICON_BUG,
  ICON_CHECKLIST,
  ICON_CHEVRON_DOWN,
  ICON_LAYERS,
} from "@workshop/look/icons";
import { Emitter, type Event } from "@workshop/platform/event";
import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import { DropdownMenu } from "@workshop/look/dropdown";
import type { DropdownItem } from "@workshop/look/dropdown";

/** The agent interaction modes, keyed by display label. */
export const UNIFIED_MODES = {
  Agent: "agent",
  Plan: "plan",
  Debug: "debug",
  Multitask: "multitask",
  Ask: "ask",
} as const;

/** An agent interaction mode. */
export type UnifiedMode = (typeof UNIFIED_MODES)[keyof typeof UNIFIED_MODES];

/** The document-level event a mode selection fires; `detail` names the mode. */
export const AGENT_MODE_CHANGED_EVENT = "agent-mode-changed";

// The labels in menu order, derived from UNIFIED_MODES so the dropdown
// can never drift from the mode set. Object.keys hides the literal key
// type; the cast restores it.
const MODE_LABELS = ["Agent", "Plan", "Debug", "Multitask", "Ask"] as const;

/** Display labels by mode value: the inverse of UNIFIED_MODES. */
const MODE_LABEL_BY_MODE: Record<UnifiedMode, string> = {
  [UNIFIED_MODES.Agent]: "Agent",
  [UNIFIED_MODES.Plan]: "Plan",
  [UNIFIED_MODES.Debug]: "Debug",
  [UNIFIED_MODES.Multitask]: "Multitask",
  [UNIFIED_MODES.Ask]: "Ask",
};

/** The line under each label in the menu, in Cursor's own words. */
const MODE_DESCRIPTION: Record<UnifiedMode, string> = {
  [UNIFIED_MODES.Agent]: "Plan, search, build anything",
  [UNIFIED_MODES.Plan]: "Create detailed plans for accomplishing tasks",
  [UNIFIED_MODES.Debug]: "Systematically diagnose and fix bugs using runtime traces",
  [UNIFIED_MODES.Multitask]: "Run and coordinate multiple tasks in parallel",
  [UNIFIED_MODES.Ask]: "Ask Cursor questions about your codebase",
};

/** Mode glyphs: static codicon strings from @workshop/look, never data. */
const MODE_ICON_HTML: Record<UnifiedMode, string> = {
  [UNIFIED_MODES.Agent]: ICON_AGENT,
  [UNIFIED_MODES.Plan]: ICON_CHECKLIST,
  [UNIFIED_MODES.Debug]: ICON_BUG,
  [UNIFIED_MODES.Multitask]: ICON_LAYERS,
  [UNIFIED_MODES.Ask]: ICON_ASK,
};

/** The chip's tooltip. */
const CHIP_TITLE = "Switch Agent Mode (Ctrl+.)";

/** The menu opens above the chip, its left edge 6px left of the chip's. */
const MENU_OFFSET_X = -6;

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
  private readonly modeEmitter = this._register(new Emitter<UnifiedMode>());
  private current: UnifiedMode = UNIFIED_MODES.Agent;

  /** Fires with the new mode each time the selection changes. */
  readonly onDidChangeMode: Event<UnifiedMode> = this.modeEmitter.event;

  constructor() {
    super();
    this.element = document.createElement("button");
    this.element.type = "button";
    this.element.className = "ws-mode-chip";
    this.element.title = CHIP_TITLE;

    this.iconSlot = document.createElement("span");
    this.iconSlot.className = "ws-mode-chip__icon";
    this.iconSlot.setAttribute("aria-hidden", "true");
    this.labelSlot = document.createElement("span");
    this.labelSlot.className = "ws-mode-chip__label";
    const chevron = document.createElement("span");
    chevron.className = "ws-mode-chip__chevron";
    chevron.setAttribute("aria-hidden", "true");
    chevron.innerHTML = ICON_CHEVRON_DOWN;
    this.element.append(this.iconSlot, this.labelSlot, chevron);

    this.dropdown = this._register(new DropdownMenu());

    const onClick = (): void => this.showMenu();
    this.element.addEventListener("click", onClick);
    this._register(
      toDisposable(() => this.element.removeEventListener("click", onClick)),
    );

    // Shift+Tab again, with the menu open and focus in it, cycles the mode:
    // the menu would otherwise treat Tab as its dismissal. Ctrl+. is a
    // registered keybinding, so it reaches openOrCycle through its command.
    const onShiftTab = (event: KeyboardEvent): void => {
      if (this.dropdown.isOpen && event.key === "Tab" && event.shiftKey) {
        event.preventDefault();
        event.stopPropagation();
        this.openOrCycle();
      }
    };
    document.addEventListener("keydown", onShiftTab, true);
    this._register(
      toDisposable(() => document.removeEventListener("keydown", onShiftTab, true)),
    );

    this.renderMode();
  }

  /** The selected mode. */
  get mode(): UnifiedMode {
    return this.current;
  }

  /**
   * Ctrl+. and Shift+Tab: opens the menu, and while it is open selects the
   * next mode in menu order (wrapping), leaving the menu open on the new
   * choice so the next press keeps cycling.
   */
  openOrCycle(): void {
    if (!this.dropdown.isOpen) {
      this.showMenu();
      return;
    }
    const index = MODE_LABELS.findIndex((label) => UNIFIED_MODES[label] === this.current);
    const next = MODE_LABELS[(index + 1) % MODE_LABELS.length];
    this.dropdown.close();
    if (next !== undefined) {
      this.select(UNIFIED_MODES[next]);
    }
    this.showMenu();
  }

  private showMenu(): void {
    const items: DropdownItem[] = MODE_LABELS.map((label) => {
      const mode = UNIFIED_MODES[label];
      return {
        label,
        description: MODE_DESCRIPTION[mode],
        iconHtml: MODE_ICON_HTML[mode],
        selected: mode === this.current,
        onClick: () => this.select(mode),
      };
    });
    this.dropdown.show(this.element, items, undefined, {
      skin: "composer",
      className: "ws-mode-menu",
      placement: "above",
      offsetX: MENU_OFFSET_X,
    });
  }

  private select(mode: UnifiedMode): void {
    // The event is "changed": re-picking the current mode stays silent.
    if (mode === this.current) {
      return;
    }
    this.current = mode;
    this.renderMode();
    this.modeEmitter.fire(mode);
    document.dispatchEvent(
      new CustomEvent<UnifiedMode>(AGENT_MODE_CHANGED_EVENT, { detail: mode }),
    );
  }

  private renderMode(): void {
    // The icon HTML is this module's own static string, never input.
    this.iconSlot.innerHTML = MODE_ICON_HTML[this.current];
    this.labelSlot.textContent = MODE_LABEL_BY_MODE[this.current];
    this.element.dataset["mode"] = this.current;
  }
}
