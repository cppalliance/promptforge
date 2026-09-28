// The dockview renderer seam for the panel registry. The panel kinds
// themselves - zone affinity, title, closability, and the import thunk
// that lazy-loads the feature directory - are registered by each feature
// into @workshop/platform/panel-registry; this file holds the DOM side:
// the LazyPanel that stands in for a panel while its chunk loads (Home
// Assistant's partial-panel-resolver pattern) and Dockview's
// createComponent / createTabComponent dispatch. main.ts and the tests
// build the dock's dispatch from here.

import type {
  CreateComponentOptions,
  GroupPanelPartInitParameters,
  IContentRenderer,
  ITabRenderer,
} from "dockview";

import { Disposable } from "@workshop/platform/lifecycle";
import { loadPanelType, panelTypeEntry } from "@workshop/platform/panel-registry";
import { PanelTab } from "./panel-tab";

export {
  isPanelType,
  panelTypeEntry,
  registerPanelFactory,
  registerPanelType,
} from "@workshop/platform/panel-registry";
export type { PanelFeatureModule, PanelType, PanelTypeEntry } from "@workshop/platform/panel-registry";

/** The registered name of the generic tab; the dock's default tab component. */
export const PANEL_TAB = "panel-tab";

/**
 * A dockview content renderer standing in for a panel whose feature
 * chunk is still loading. The element mounts into the dock immediately
 * (a placeholder keeps the layout stable); when the thunk resolves, the
 * directory's register() has run and the real panel's element swaps in,
 * receiving the init parameters dockview delivered at mount, plus the
 * last dimensions the dock laid the placeholder out at. Disposing before the
 * load resolves cancels the swap. The placeholder's sizing (a full-height flex
 * column, .ws-panel-lazy in zones.css) is what lets the real panel's
 * `height: 100%` resolve against the dock's content container.
 */
class LazyPanel extends Disposable implements IContentRenderer {
  readonly element = document.createElement("div");
  private inner: (IContentRenderer & { dispose?: () => void }) | null = null;
  private dimension: readonly [width: number, height: number] | null = null;
  private unloaded = false;

  constructor(private readonly type: string) {
    super();
    this.element.className = "ws-panel-lazy";
    this.element.dataset["panelType"] = type;
  }

  init(parameters: GroupPanelPartInitParameters): void {
    void loadPanelType(this.type)
      .then((factory) => {
        if (this.unloaded) {
          return;
        }
        if (factory === undefined) {
          this.showError(`Unknown panel: ${this.type}`);
          return;
        }
        const renderer = factory();
        this.inner = renderer;
        this.element.appendChild(renderer.element);
        renderer.init(parameters);
        if (this.dimension !== null) {
          renderer.layout?.(...this.dimension);
        }
      })
      .catch((error: unknown) => {
        if (!this.unloaded) {
          this.showError(error instanceof Error ? error.message : String(error));
        }
      });
  }

  /** The real panel once the feature chunk has resolved; null before. */
  get resolvedPanel(): IContentRenderer | null {
    return this.inner;
  }

  /**
   * Forwards the dock's resize to the real panel; a resize that lands
   * before the chunk resolves is replayed at the swap.
   */
  layout(width: number, height: number): void {
    this.dimension = [width, height];
    this.inner?.layout?.(width, height);
  }

  private showError(message: string): void {
    const element = document.createElement("div");
    element.className = "ws-panel-error";
    element.setAttribute("role", "alert");
    element.textContent = message;
    this.element.replaceChildren(element);
  }

  override dispose(): void {
    this.unloaded = true;
    this.inner?.dispose?.();
    this.inner = null;
    super.dispose();
  }
}

/**
 * Dockview's createComponent dispatch: component name -> a lazy renderer
 * for the registered panel kind. Unknown names should never arrive -
 * every addPanel call goes through openInZone with a registered type -
 * but an unknown name must not break the dock, so it renders a labelled
 * placeholder instead of throwing.
 */
export function createPanelComponent(options: CreateComponentOptions): IContentRenderer {
  if (panelTypeEntry(options.name) !== undefined) {
    return new LazyPanel(options.name);
  }
  const element = document.createElement("div");
  element.className = "ws-panel-unknown";
  element.textContent = `Unknown panel: ${options.name}`;
  return { element, init: () => undefined };
}

/**
 * Dockview's createTabComponent dispatch. Returning undefined for any
 * other name makes Dockview fall back to its default closable tab.
 */
export function createPanelTabComponent(options: CreateComponentOptions): ITabRenderer | undefined {
  return options.name === PANEL_TAB ? new PanelTab() : undefined;
}
