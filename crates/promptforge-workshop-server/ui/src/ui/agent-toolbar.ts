// The agent toolbar: a flex row composing the mode chip, the model
// picker trigger, and the token ring. The chip and picker lead from the
// inline-start edge; the ring is the last child so the stylesheet can
// pin it to the trailing edge. Composition only - each child owns its
// behavior; the toolbar owns their lifetimes and the container's
// semantics.

import "./agent-toolbar.css";

import { Disposable } from "../base/lifecycle";
import type { ModelService } from "../services/model-service";
import { ModeChip } from "./mode-chip";
import { ModelPickerTrigger } from "./model-picker-trigger";
import { TokenRing } from "./token-ring";

/**
 * The toolbar row. Disposable: dispose() disposes all three children.
 * The model service is borrowed, not owned - the composition root
 * disposes it.
 */
export class AgentToolbar extends Disposable {
  /** The toolbar container; append it where the toolbar belongs. */
  readonly element: HTMLElement;

  constructor(modelService: ModelService) {
    super();

    this.element = document.createElement("div");
    this.element.className = "agent-toolbar";
    this.element.setAttribute("role", "toolbar");
    this.element.setAttribute("aria-label", "Agent controls");

    const modeChip = this._register(new ModeChip());
    const modelPicker = this._register(new ModelPickerTrigger(modelService));
    const tokenRing = this._register(new TokenRing());

    this.element.append(modeChip.element, modelPicker.element, tokenRing.element);
  }
}
