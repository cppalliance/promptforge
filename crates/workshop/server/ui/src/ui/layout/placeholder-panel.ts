// The placeholder panel: the inert content a zone's resurrected group
// holds after its last real panel closes, so the zone's space survives
// (zones.ts' resurrection is the only producer). It reads its zone from
// the panel params to pick the hint text; it has no actions, no state,
// and no close button (its tab is the permanent renderer), and it is
// never given zone overrides - opening a real panel into the zone
// replaces it.

import type { GroupPanelPartInitParameters } from "dockview";

import { WorkshopPart } from "../../base/workshop-part";

/** The inert hint per zone. */
const HINTS: Record<string, string> = {
  left: "Open the Workshop tree to begin",
  main: "Open a file to begin",
  right: "Open an agent session to begin",
};

export class PlaceholderPanel extends WorkshopPart {
  private zone: string | null = null;

  constructor() {
    super();
    this.element.className = "ws-placeholder-panel";
  }

  override init(parameters: GroupPanelPartInitParameters): void {
    const zone = parameters.params?.zone;
    this.zone = typeof zone === "string" ? zone : null;
    super.init(parameters);
  }

  protected create(parent: HTMLElement): void {
    const hint = document.createElement("p");
    hint.className = "ws-placeholder-panel__hint";
    hint.textContent = (this.zone !== null && HINTS[this.zone]) || "Nothing here yet";
    parent.appendChild(hint);
  }
}
