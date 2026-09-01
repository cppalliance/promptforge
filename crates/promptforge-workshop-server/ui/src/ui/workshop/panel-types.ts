// The static panel registry: every Dockview panel kind declared once with
// its zone affinity, default title, content factory, and optional tab
// renderer. zones.ts resolves placement from `defaultZone`; main.ts and
// the tests build Dockview's createComponent / createTabComponent dispatch
// from here. Adding a panel kind means adding one entry.

import type { CreateComponentOptions, IContentRenderer, ITabRenderer, TabPartInitParameters } from "dockview";

import { Disposable } from "../../base/lifecycle";
import { AgentPanel } from "./agent-panel";
import { ChatPanel } from "./chat-panel";
import { EditorPanel } from "./editor-panel";
import { GatewayConfigPanel } from "./gateway-config-panel";
import { WorkshopTreePanel, type TreeStatusSink } from "./workshop-panel";
import type { ZoneName } from "./zones";

/**
 * The composition-root services a panel factory may consume. main.ts
 * passes them through Dockview's createComponent seam, since panel
 * params hold only serializable identity.
 */
export interface PanelServices {
  readonly statusBar: TreeStatusSink;
}

/** One panel kind's static registration. */
export interface PanelTypeEntry {
  readonly type: string;
  /** The zone a new panel opens in when the user has not moved it. */
  readonly defaultZone: ZoneName;
  readonly title: string;
  /** The named tab renderer, or undefined for Dockview's default tab. */
  readonly tabComponent: string | undefined;
  readonly factory: (services?: PanelServices) => IContentRenderer;
}

/** The registered name of the close-button-free tab renderer. */
export const PERMANENT_TAB = "permanent";

export const PANEL_TYPES = {
  tree: {
    type: "tree",
    defaultZone: "left",
    title: "Workshop",
    // The Workshop tree anchors the workbench; its tab has no close
    // button, so the panel cannot be dismissed from the tab strip.
    tabComponent: PERMANENT_TAB,
    factory: (services?: PanelServices): IContentRenderer =>
      new WorkshopTreePanel(services?.statusBar ?? null),
  },
  editor: {
    type: "editor",
    defaultZone: "main",
    title: "Editor",
    tabComponent: undefined,
    factory: (): IContentRenderer => new EditorPanel(),
  },
  chat: {
    type: "chat",
    defaultZone: "right",
    title: "Agent",
    tabComponent: undefined,
    factory: (): IContentRenderer => new ChatPanel(),
  },
  config: {
    type: "config",
    defaultZone: "main",
    title: "Gateway Config",
    tabComponent: undefined,
    factory: (): IContentRenderer => new GatewayConfigPanel(),
  },
  agent: {
    type: "agent",
    defaultZone: "right",
    title: "Agent Session",
    tabComponent: undefined,
    factory: (): IContentRenderer => new AgentPanel(),
  },
} as const satisfies Record<string, PanelTypeEntry>;

export type PanelType = keyof typeof PANEL_TYPES;

/** Narrows a Dockview component name to a registered panel type. */
export function isPanelType(name: string): name is PanelType {
  return Object.hasOwn(PANEL_TYPES, name);
}

/**
 * Dockview's createComponent dispatch: component name -> registered
 * factory. Unknown names should never arrive - every addPanel call goes
 * through openInZone with a registered type - but an unknown name must not
 * break the dock, so it renders a labelled placeholder instead of throwing.
 */
export function createPanelComponent(
  options: CreateComponentOptions,
  services?: PanelServices,
): IContentRenderer {
  if (isPanelType(options.name)) {
    return PANEL_TYPES[options.name].factory(services);
  }
  const element = document.createElement("div");
  element.className = "panel-unknown";
  element.textContent = `Unknown panel: ${options.name}`;
  return { element, init: () => undefined };
}

/**
 * The tab for panels that must never be closed from the tab strip: the
 * default chip's structure (same classes, so the theme styles it
 * identically) minus the close action.
 */
class PermanentTab extends Disposable implements ITabRenderer {
  public readonly element = document.createElement("div");
  private readonly content = document.createElement("div");

  constructor() {
    super();
    this.element.className = "dv-default-tab";
    this.content.className = "dv-default-tab-content";
    this.element.appendChild(this.content);
  }

  public init(parameters: TabPartInitParameters): void {
    this.content.textContent = parameters.title;
    // Dockview calls dispose() when the tab is removed; the inherited
    // Disposable dispose releases this subscription.
    this._register(
      parameters.api.onDidTitleChange((event) => {
        this.content.textContent = event.title;
      }),
    );
  }
}

/**
 * Dockview's createTabComponent dispatch. Returning undefined for any
 * other name (including panels that never named a tab component) makes
 * Dockview fall back to its default closable tab.
 */
export function createPanelTabComponent(options: CreateComponentOptions): ITabRenderer | undefined {
  return options.name === PERMANENT_TAB ? new PermanentTab() : undefined;
}
