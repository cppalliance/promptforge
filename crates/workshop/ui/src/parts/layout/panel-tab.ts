// The one tab renderer every dock panel uses: the default chip's structure
// (same classes, so the theme styles it identically) driven by the panel
// type's registry entry. A closable type gets the close action, which calls
// the layout core's close path (panel-close.ts) with the tab's { panelId }
// so the part confirms first, and a right-click menu over
// MenuId.EditorTitleContext at the pointer, whose rows run against the
// clicked tab and read activeEditor as its type; a `closable: false` type
// gets neither. Delete and Backspace on the focused tab make the same
// call, and do nothing on a `closable: false` type.
//
// The loading shimmer: while its panel is loading, the title span takes
// @workshop/look's .ws-shimmer-text; the negative animation-delay against
// the module-level epoch keeps the sweep continuous across the re-renders
// a tab title goes through (the same trick upstream VS Code uses). Header
// tabs register themselves by panel id in the module map below on init
// and remove themselves on dispose; a panel drives its shimmer through
// setTabLoading without holding a reference to the tab. Dockview's
// overflow list inits a second renderer per overflowed panel under the
// same id and never disposes it, so only the "header" renderer holds the
// panel's slot.

import type { ITabRenderer, TabPartInitParameters } from "dockview";

import { Disposable, toDisposable } from "@workshop/platform/lifecycle";
import { MenuId } from "@workshop/platform/menu-registry";
import { panelTypeEntry } from "@workshop/platform/panel-registry";
// The widget module, never the parts/menu barrel: the barrel imports the
// contribution surface, and lazy panels import this module for
// setTabLoading.
import { Menu, reportCommandFailure } from "../menu/menu";
import { closeActiveEditor } from "./panel-close";

/** Closes one panel through the layout core, so its part confirms before it goes. */
function closePanel(panelId: string): void {
  void closeActiveEditor({ panelId }).catch((error: unknown) => {
    reportCommandFailure("workbench.action.closeActiveEditor", error);
  });
}

/** The shimmer period, matching the 2s loop in @workshop/look/shimmer.css. */
const SHIMMER_PERIOD_MS = 2000;
// The epoch the negative animation-delay is computed against, so a
// re-rendered title continues the sweep instead of restarting it.
const SHIMMER_EPOCH = Date.now();

/** The live tabs, by panel id. */
const tabs = new Map<string, PanelTab>();
// The desired shimmer state per panel id, so a tab that mounts after
// its panel started loading still picks the shimmer up; entries die
// with their tab.
const loadingByPanel = new Map<string, boolean>();

/**
 * Drives one tab's shimmer. A panel calls this on every state
 * transition; a tab that has not mounted yet picks the state up at
 * init, and one already gone is a no-op.
 */
export function setTabLoading(panelId: string, loading: boolean): void {
  loadingByPanel.set(panelId, loading);
  tabs.get(panelId)?.setLoading(loading);
}

export class PanelTab extends Disposable implements ITabRenderer {
  public readonly element = document.createElement("div");
  private readonly content = document.createElement("div");
  private menu: Menu | null = null;
  private panelId: string | null = null;
  private loading = false;

  constructor() {
    super();
    this.element.className = "dv-default-tab";
    this.content.className = "dv-default-tab-content";
    this.element.appendChild(this.content);
  }

  public init(parameters: TabPartInitParameters): void {
    const { api } = parameters;
    const closable = panelTypeEntry(api.component)?.closable !== false;
    if (parameters.tabLocation === "header") {
      this.panelId = api.id;
      tabs.set(api.id, this);
      this.interceptCloseKeys(api.id, closable);
    }
    // The panel may have entered loading before the tab mounted.
    this.loading = loadingByPanel.get(api.id) ?? false;
    this.content.textContent = parameters.title;
    this._register(
      api.onDidTitleChange((event) => {
        this.content.textContent = event.title;
      }),
    );
    this.applyLoading();
    if (!closable) {
      return;
    }
    const close = document.createElement("button");
    close.type = "button";
    close.className = "dv-default-tab-action";
    close.setAttribute("aria-label", "Close");
    close.textContent = "×";
    close.tabIndex = -1;
    // Dockview's tab wrapper activates the panel and its group on
    // pointerdown unless the event is default-prevented, as its own
    // DefaultTab action does.
    close.addEventListener("pointerdown", (event) => {
      event.preventDefault();
    });
    close.addEventListener("click", (event) => {
      event.stopPropagation();
      closePanel(api.id);
    });
    this.element.appendChild(close);
    this.element.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      event.stopPropagation();
      this.menu ??= this._register(new Menu());
      this.menu.open(
        MenuId.EditorTitleContext,
        { x: event.clientX, y: event.clientY },
        { panelId: api.id },
        { activeEditor: api.component },
      );
    });
  }

  /**
   * Takes Delete and Backspace on the focused tab away from Dockview.
   * dockview-core 8.3.1's tab strip (Tabs._onKeyDown in
   * dist/package/main.esm.mjs) closes the focused tab on either key
   * through panel.api.close(), without checking `closable` and without
   * the part's confirmClose. The focused element is the Dockview wrapper
   * this renderer is appended into, which Dockview replaces when the
   * panel moves groups and never hands to the renderer, so the keys are
   * caught in the document's capture phase and only while that wrapper
   * is the target, and call the layout core's close path directly.
   */
  private interceptCloseKeys(panelId: string, closable: boolean): void {
    const onKeydown = (event: KeyboardEvent): void => {
      const wrapper = this.element.parentElement;
      if ((event.key !== "Delete" && event.key !== "Backspace") || wrapper === null || event.target !== wrapper) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (closable) {
        closePanel(panelId);
      }
    };
    document.addEventListener("keydown", onKeydown, true);
    this._register(toDisposable(() => document.removeEventListener("keydown", onKeydown, true)));
  }

  /** Toggles the shimmer on the title span. */
  public setLoading(loading: boolean): void {
    this.loading = loading;
    this.applyLoading();
  }

  private applyLoading(): void {
    if (this.loading) {
      this.content.classList.add("ws-shimmer-text");
      this.content.style.animationDelay = `-${((Date.now() - SHIMMER_EPOCH) % SHIMMER_PERIOD_MS + SHIMMER_PERIOD_MS) % SHIMMER_PERIOD_MS}ms`;
    } else {
      this.content.classList.remove("ws-shimmer-text");
      this.content.style.animationDelay = "";
    }
  }

  public override dispose(): void {
    if (this.panelId !== null) {
      tabs.delete(this.panelId);
      loadingByPanel.delete(this.panelId);
      this.panelId = null;
    }
    super.dispose();
  }
}
