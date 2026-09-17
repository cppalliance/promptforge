// The dockview renderer seam for the panel registry. The panel kinds
// themselves - zone affinity, title, tab renderer, and the import thunk
// that lazy-loads the feature directory - are declared in
// services/panel-registry.ts; this file holds the DOM side: the LazyPanel
// that stands in for a panel while its chunk loads (Home Assistant's
// partial-panel-resolver pattern), the tab renderers, and Dockview's
// createComponent / createTabComponent dispatch. main.ts and the tests
// build the dock's dispatch from here.

import type {
  CreateComponentOptions,
  GroupPanelPartInitParameters,
  IContentRenderer,
  ITabRenderer,
  TabPartInitParameters,
} from "dockview";

import { Disposable } from "../../base/lifecycle";
import {
  AGENT_TAB,
  PERMANENT_TAB,
  loadPanelType,
  panelTypeEntry,
} from "../../services/panel-registry";
import { DropdownMenu } from "shared-ui/dropdown";

export {
  AGENT_TAB,
  PERMANENT_TAB,
  isPanelType,
  panelTypeEntry,
  registerPanelFactory,
  registerPanelType,
} from "../../services/panel-registry";
export type { PanelFeatureModule, PanelType, PanelTypeEntry } from "../../services/panel-registry";

/**
 * A dockview content renderer standing in for a panel whose feature
 * chunk is still loading. The element mounts into the dock immediately
 * (an empty shell keeps the layout stable); when the thunk resolves, the
 * directory's register() has run and the real panel's element swaps in,
 * receiving the init parameters dockview delivered at mount, plus the
 * last dimensions the dock laid the shell out at. Disposing before the
 * load resolves cancels the swap. The shell's sizing (a full-height flex
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

class AgentTab extends Disposable implements ITabRenderer {
  public readonly element = document.createElement("div");
  private readonly content = document.createElement("div");
  private readonly close = document.createElement("button");
  private readonly menu = this._register(new DropdownMenu());

  constructor() {
    super();
    this.element.className = "dv-default-tab";
    this.content.className = "dv-default-tab-content";
    this.close.type = "button";
    this.close.className = "dv-default-tab-action";
    this.close.setAttribute("aria-label", "Close");
    this.close.textContent = "×";
    this.element.append(this.content, this.close);
  }

  public init(parameters: TabPartInitParameters): void {
    this.content.textContent = parameters.title;
    this._register(
      parameters.api.onDidTitleChange((event) => {
        this.content.textContent = event.title;
      }),
    );
    this.close.addEventListener("click", (event) => {
      event.stopPropagation();
      parameters.api.close();
    });
    this.element.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      event.stopPropagation();
      this.menu.show(
        this.element,
        [
          { label: "Close", onClick: () => parameters.api.close() },
          {
            label: "Close Others",
            onClick: () => {
              for (const panel of [...parameters.api.group.panels]) {
                if (panel.api.id !== parameters.api.id) {
                  panel.api.close();
                }
              }
            },
          },
        ],
        { x: event.clientX, y: event.clientY },
      );
    });
  }
}

/**
 * Dockview's createTabComponent dispatch. Returning undefined for any
 * other name (including panels that never named a tab component) makes
 * Dockview fall back to its default closable tab.
 */
export function createPanelTabComponent(options: CreateComponentOptions): ITabRenderer | undefined {
  if (options.name === PERMANENT_TAB) {
    return new PermanentTab();
  }
  return options.name === AGENT_TAB ? new AgentTab() : undefined;
}
