// The agent toolbar: a flex row composing the mode chip, the model
// picker trigger, and the token ring. The chip and picker lead from the
// inline-start edge; the ring is the last child so the stylesheet can
// pin it to the trailing edge, and the chat box's round action button
// joins the row after it. Composition only - each child owns its
// behavior; the toolbar owns their lifetimes and the container's
// semantics, and forwards the two menu openers the composer's keys use
// (Ctrl+. and Shift+Tab for the mode menu, Ctrl+/ for the model menu).

import "./agent-toolbar.css";

import type { Event } from "@workshop/platform/event";
import { Disposable } from "@workshop/platform/lifecycle";
import type { ModelService } from "../../services/model-service";
import { ModeChip, type UnifiedMode } from "./mode-chip";
import { ModelPickerTrigger } from "../chrome/model-picker-trigger";
import { TokenRing } from "../chrome/token-ring";

/**
 * The toolbar row. Disposable: dispose() disposes all three children.
 * The model service is borrowed, not owned - the composition root
 * disposes it.
 */
export class AgentToolbar extends Disposable {
  /** The toolbar container; append it where the toolbar belongs. */
  readonly element: HTMLElement;

  private readonly modeChip: ModeChip;
  private readonly modelPicker: ModelPickerTrigger;

  constructor(modelService: ModelService) {
    super();

    this.element = document.createElement("div");
    this.element.className = "ws-agent-toolbar";
    this.element.setAttribute("role", "toolbar");
    this.element.setAttribute("aria-label", "Agent controls");

    this.modeChip = this._register(new ModeChip());
    this.modelPicker = this._register(new ModelPickerTrigger(modelService));
    const tokenRing = this._register(new TokenRing());

    this.element.append(this.modeChip.element, this.modelPicker.element, tokenRing.element);
  }

  /** The selected agent mode. */
  get mode(): UnifiedMode {
    return this.modeChip.mode;
  }

  /** Fires with the new mode each time the selection changes. */
  get onDidChangeMode(): Event<UnifiedMode> {
    return this.modeChip.onDidChangeMode;
  }

  /** Opens the mode menu; while it is open, cycles to the next mode. */
  openModeMenu(): void {
    this.modeChip.openOrCycle();
  }

  /** Opens the model menu. */
  openModelMenu(): void {
    this.modelPicker.open();
  }
}
