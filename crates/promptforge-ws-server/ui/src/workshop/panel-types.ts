// The static panel registry: every Dockview panel kind declared once with
// its zone affinity, default title, and content factory. zones.ts resolves
// placement from `defaultZone`; main.ts and the tests build Dockview's
// createComponent dispatch from `factory`. Adding a panel kind means
// adding one entry here.

import type { CreateComponentOptions, IContentRenderer } from "dockview";

import { ChatPanel } from "./chat-panel";
import { EditorPanel } from "./editor-panel";
import { WorkshopTreePanel } from "./workshop-panel";
import type { ZoneName } from "./zones";

/** One panel kind's static registration. */
export interface PanelTypeEntry {
  readonly type: string;
  /** The zone a new panel opens in when the user has not moved it. */
  readonly defaultZone: ZoneName;
  readonly title: string;
  readonly factory: () => IContentRenderer;
}

export const PANEL_TYPES = {
  tree: {
    type: "tree",
    defaultZone: "left",
    title: "Workshop",
    factory: (): IContentRenderer => new WorkshopTreePanel(),
  },
  editor: {
    type: "editor",
    defaultZone: "main",
    title: "Editor",
    factory: (): IContentRenderer => new EditorPanel(),
  },
  chat: {
    type: "chat",
    defaultZone: "right",
    title: "Agent",
    factory: (): IContentRenderer => new ChatPanel(),
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
export function createPanelComponent(options: CreateComponentOptions): IContentRenderer {
  if (isPanelType(options.name)) {
    return PANEL_TYPES[options.name].factory();
  }
  const element = document.createElement("div");
  element.className = "panel-unknown";
  element.textContent = `Unknown panel: ${options.name}`;
  return { element, init: () => undefined };
}
