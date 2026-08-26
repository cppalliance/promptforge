// The Agent panel controller: owns one ChatUI per Agent tab. Panels are
// created through zones.ts (openAgentPanel or a restored layout); this
// controller observes the dock, mounts a ChatUI onto each Agent panel's
// cloned .mur-app surface as it appears, and destroys it when the panel
// closes. All agents share one socket provider (the workshop socket
// multiplexes chat streams by request id) and one model selection, which
// the controller broadcasts to every live agent's engine. Voice,
// thinking, and tool state stay per-tab because the plugins factory runs
// once per ChatUI.

import type { DockviewApi, IDockviewPanel } from "dockview";

import type { ChatPlugin } from "../chat/core/types";
import { ChatUI } from "../chat/main";
import { MemoryStorage } from "../memory-storage";
import type { WorkshopProvider } from "../workshop-provider";
import { ChatPanel } from "./chat-panel";
import { openAgentPanel } from "./zones";

export interface AgentControllerOptions {
  readonly dock: DockviewApi;
  readonly provider: WorkshopProvider;
  /** Runs once per Agent tab, so each tab gets isolated plugin state. */
  readonly plugins: () => ChatPlugin[];
  /** Reads the shared model selection for newly mounted agents. */
  readonly getModel: () => string;
}

export class AgentController {
  private readonly agents = new Map<string, ChatUI>();
  private activeId: string | null = null;

  constructor(private readonly options: AgentControllerOptions) {
    const { dock } = options;
    dock.onDidAddPanel((panel) => this.mount(panel));
    dock.onDidRemovePanel((panel) => this.unmount(panel));
    dock.onDidActivePanelChange(({ panel }) => {
      if (panel !== undefined && this.agents.has(panel.id)) {
        this.activeId = panel.id;
      }
    });
    // Panels added before construction (none in the boot order, but the
    // controller must not depend on it) mount through the same path.
    for (const panel of dock.panels) {
      this.mount(panel);
    }
  }

  /** File > New Agent: a fresh tab, never a cleared conversation. */
  newAgent(): void {
    openAgentPanel();
  }

  /**
   * File > New Chat: a fresh session on the active agent. With no agent
   * open at all, opens one first so the command always lands somewhere.
   */
  newChat(): void {
    if (this.activeId === null || !this.agents.has(this.activeId)) {
      this.newAgent();
    }
    const chat = this.active();
    if (chat === null) {
      return;
    }
    void chat.engine.sessions.create().catch((error: unknown) => {
      console.error("New Chat failed:", error);
    });
  }

  /** Broadcasts a model selection to every live agent's engine. */
  applyModel(model: string): void {
    for (const chat of this.agents.values()) {
      chat.engine.setRequestDefaults({ options: { model } });
    }
  }

  /** Guarantees at least one Agent tab; used by boot after a restore. */
  ensureAgent(): void {
    if (this.agents.size === 0) {
      this.newAgent();
    }
  }

  /** The active agent's ChatUI, or null when no Agent tab is active. */
  active(): ChatUI | null {
    if (this.activeId === null) {
      return null;
    }
    return this.agents.get(this.activeId) ?? null;
  }

  private mount(panel: IDockviewPanel): void {
    const content = panel.view.content;
    if (!(content instanceof ChatPanel) || this.agents.has(panel.id)) {
      return;
    }
    const container = content.element.querySelector(".mur-app");
    if (!(container instanceof HTMLElement)) {
      throw new Error("DOM Error: an Agent panel did not mount its .mur-app container.");
    }
    const chat = new ChatUI({
      container,
      provider: this.options.provider,
      storage: new MemoryStorage(),
      enableSidebar: false,
      routing: false,
      fullscreen: false,
      plugins: this.options.plugins,
    });
    chat.engine.setRequestDefaults({ options: { model: this.options.getModel() } });
    this.agents.set(panel.id, chat);
    if (this.activeId === null || panel.api.isActive) {
      this.activeId = panel.id;
    }
  }

  private unmount(panel: IDockviewPanel): void {
    const chat = this.agents.get(panel.id);
    if (chat === undefined) {
      return;
    }
    this.agents.delete(panel.id);
    if (this.activeId === panel.id) {
      const active = this.options.dock.activePanel;
      this.activeId =
        active !== undefined && this.agents.has(active.id)
          ? active.id
          : (this.agents.keys().next().value ?? null);
    }
    void chat.destroy().catch((error: unknown) => {
      console.error("destroying an Agent tab failed:", error);
    });
  }
}
