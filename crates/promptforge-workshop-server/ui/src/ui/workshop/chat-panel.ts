// The agent chat surface as a Dockview panel. The panel clones the
// #chat-panel template from index.html; main.ts mounts the real ChatUI
// onto the cloned .mur-app container once the panel is added.

import type { IContentRenderer } from "dockview";

export class ChatPanel implements IContentRenderer {
  readonly element = document.createElement("div");

  constructor() {
    this.element.className = "chat-panel";
  }

  init(): void {
    const template = document.getElementById("chat-panel");
    if (!(template instanceof HTMLTemplateElement)) {
      throw new Error("DOM Error: #chat-panel template missing from the page.");
    }
    this.element.appendChild(template.content.cloneNode(true));
  }
}
