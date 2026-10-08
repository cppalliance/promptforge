// The model picker trigger: a toolbar button showing the selected
// model's id, with a 9px chevron. Clicking it (or Ctrl+/, through open)
// opens a DropdownMenu of the ModelService catalog on the composer's menu
// surface - check-only selection, 230px wide, at most 320px tall - and
// picking one sends the select command through the service. The label
// never updates optimistically - the server owns the selection, so the
// trigger re-renders only when the service's selection changes. The label
// shows the selected id alone, so a catalog change leaves it as it is; the
// menu reads the catalog each time it opens.

import "./model-picker-trigger.css";

import { ICON_CHEVRON_DOWN } from "@workshop/look/icons";
import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import type { ModelService } from "../../services/model-service";
import { DropdownMenu } from "@workshop/look/dropdown";
import type { DropdownItem } from "@workshop/look/dropdown";

/** The trigger label when no model is selected. */
const NO_SELECTION_LABEL = "Select model";

/** The single menu row shown when the catalog is empty. */
const EMPTY_CATALOG_LABEL = "No models found";

/** The trigger's tooltip. */
const TRIGGER_TITLE = "Switch Model (Ctrl+/)";

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
    this.element.className = "ws-model-picker-trigger";
    this.element.title = TRIGGER_TITLE;

    this.labelSlot = document.createElement("span");
    this.labelSlot.className = "ws-model-picker-trigger__label";

    const iconSlot = document.createElement("span");
    iconSlot.className = "ws-model-picker-trigger__icon";
    iconSlot.setAttribute("aria-hidden", "true");
    // The chevron is a static string from @workshop/look, never input.
    iconSlot.innerHTML = ICON_CHEVRON_DOWN;

    this.element.append(this.labelSlot, iconSlot);

    this.dropdown = this._register(new DropdownMenu());

    const onClick = (): void => this.showMenu();
    this.element.addEventListener("click", onClick);
    this._register(
      toDisposable(() => this.element.removeEventListener("click", onClick)),
    );

    this._register(this.modelService.onDidChangeCurrent(() => this.renderCurrent()));

    this.renderCurrent();
  }

  /** Opens the model menu (Ctrl+/); a menu already open stays open. */
  open(): void {
    if (!this.dropdown.isOpen) {
      this.showMenu();
    }
  }

  private showMenu(): void {
    const models = this.modelService.models;
    const current = this.modelService.current;
    const items: DropdownItem[] =
      models.length === 0
        ? [{ label: EMPTY_CATALOG_LABEL, onClick: () => {} }]
        : models.map((model) => ({
            label: model.id,
            selected: model.id === current,
            onClick: () => this.select(model.id),
          }));
    this.dropdown.show(this.element, items, undefined, {
      skin: "composer",
      className: "ws-model-menu",
      placement: "above",
    });
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
  }
}
