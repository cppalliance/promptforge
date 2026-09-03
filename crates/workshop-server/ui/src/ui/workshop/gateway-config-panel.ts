// The Gateway Config panel: a Dockview panel hosting the gateway's
// config SPA in an iframe at /gateway/config/?mode=panel, proxied
// same-origin through the workshop server. The panel only hosts; all
// traffic between the iframe and the workshop flows through the
// window-level bridge in gateway-config-bridge.ts. The workshop's own
// origin rides along in the iframe URL's `bridge` parameter, so the
// iframe can pin its postMessage targetOrigin to the real parent
// instead of "*".

import "./gateway-config-panel.css";

import type { GroupPanelPartInitParameters, IContentRenderer } from "dockview";

import { Disposable } from "../../base/lifecycle";

/** Injectable seams for tests. */
export interface GatewayConfigPanelDeps {
  /** The workshop's own origin; window.location.origin in production. */
  readonly workshopOrigin?: string;
}

export class GatewayConfigPanel extends Disposable implements IContentRenderer {
  readonly element = document.createElement("div");

  constructor(private readonly deps: GatewayConfigPanelDeps = {}) {
    super();
    this.element.className = "gateway-config-panel";
  }

  init(_parameters: GroupPanelPartInitParameters): void {
    const iframe = document.createElement("iframe");
    iframe.className = "gateway-config-panel__frame";
    iframe.title = "Gateway Config";
    // The config SPA is proxied through the workshop server at
    // /gateway/config/ so the iframe is same-origin (a cross-origin
    // iframe to the gateway's port made Chromium spawn renderer
    // processes that flashed a console window on Windows). allow-scripts
    // runs the SPA; allow-same-origin keeps it on the workshop origin.
    iframe.setAttribute("sandbox", "allow-scripts allow-same-origin");
    const workshopOrigin = this.deps.workshopOrigin ?? window.location.origin;
    iframe.src = `/gateway/config/?mode=panel&bridge=${encodeURIComponent(workshopOrigin)}`;
    this.element.replaceChildren(iframe);
  }
}
