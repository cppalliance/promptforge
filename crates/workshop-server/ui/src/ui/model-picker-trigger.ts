// The model picker trigger: a pill-shaped toolbar button showing the
// selected model's id. Clicking opens a DropdownMenu of the ModelService
// catalog; picking one sends the select command through the service. The
// label never updates optimistically - the server owns the selection, so
// the trigger re-renders only when the service's change events fire. The
// catalog subscription earns its keep through the tooltip: the current
// model's description, resolved from the latest catalog (the title-bar
// Model menu's convention).

import "./model-picker-trigger.css";

import { ChevronDown, createElement } from "lucide";
import { Disposable, toDisposable } from "../base/lifecycle";
import type { ModelService } from "../services/model-service";
import { DropdownMenu } from "shared-ui/dropdown";
import type { DropdownItem } from "shared-ui/dropdown";

/** The trigger label when no model is selected. */
const NO_SELECTION_LABEL = "Select model";

/** The single menu row shown when the catalog is empty. */
const EMPTY_CATALOG_LABEL = "No models available";

/**
 * The trigger button plus its dropdown. Disposable: dispose() closes an
 * open menu, removes the click listener, and unsubscribes from the
 * service. The service is borrowed, not owned - the composition root
 * disposes it.
 */
export class ModelPickerTrigger extends Disposable {
  /** The trigger button; append it where the picker belongs. */
  readonly element: HTMLButtonElement;

  private readonly dropdown: DropdownMenu;
  private readonly labelSlot: HTMLSpanElement;

  constructor(private readonly modelService: ModelService) {
    super();

    this.element = document.createElement("button");
    this.element.type = "button";
    this.element.className = "model-picker-trigger";

    this.labelSlot = document.createElement("span");
    this.labelSlot.className = "model-picker-trigger__label";

    const iconSlot = document.createElement("span");
    iconSlot.className = "model-picker-trigger__icon";
    iconSlot.setAttribute("aria-hidden", "true");
    // The chevron is this module's own static markup, never input.
    iconSlot.innerHTML = createElement(ChevronDown, { width: 12, height: 12 }).outerHTML;

    this.element.append(this.labelSlot, iconSlot);

    this.dropdown = this._register(new DropdownMenu());

    const onClick = (): void => this.showMenu();
    this.element.addEventListener("click", onClick);
    this._register(
      toDisposable(() => this.element.removeEventListener("click", onClick)),
    );

    this._register(this.modelService.onDidChangeCurrent(() => this.renderCurrent()));
    this._register(this.modelService.onDidChangeModels(() => this.renderCurrent()));

    this.renderCurrent();
  }

  private showMenu(): void {
    const models = this.modelService.models;
    const items: DropdownItem[] =
      models.length === 0
        ? [{ label: EMPTY_CATALOG_LABEL, onClick: () => {} }]
        : models.map((model) => ({
            label: model.id,
            onClick: () => this.select(model.id),
          }));
    this.dropdown.show(this.element, items);
  }

  private select(id: string): void {
    // The send result needs no local handling: the selection changes only
    // when the server's snapshot arrives, so a failed send simply leaves
    // the label unchanged.
    this.modelService.setCurrent(id);
  }

  private renderCurrent(): void {
    const current = this.modelService.current;
    this.labelSlot.textContent = current === "" ? NO_SELECTION_LABEL : current;
    const description =
      current === ""
        ? undefined
        : this.modelService.models.find((model) => model.id === current)?.description;
    if (description === undefined) {
      this.element.removeAttribute("title");
    } else {
      this.element.title = description;
    }
  }
}
