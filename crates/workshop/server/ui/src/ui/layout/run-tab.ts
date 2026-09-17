// The Run window's tab renderer: the default chip's structure (same
// classes, so the theme styles it identically) with a close action,
// plus the loading shimmer. While its panel is loading, the title span
// carries shared-ui's .ws-shimmer-text; the negative animation-delay
// against the module-level epoch keeps the sweep continuous across the
// re-renders a tab title goes through (the same trick upstream VS Code
// uses). Tabs register themselves by panel id in the module map below
// on init and remove themselves on dispose; the RunPanel drives the
// shimmer through setRunTabLoading on every state transition, without
// holding a reference to the tab.

import type { ITabRenderer, TabPartInitParameters } from "dockview";

import { Disposable } from "../../base/lifecycle";

/** The shimmer period, matching the 2s loop in shared-ui/shimmer.css. */
const SHIMMER_PERIOD_MS = 2000;
// The epoch the negative animation-delay is computed against, so a
// re-rendered title continues the sweep instead of restarting it.
const SHIMMER_EPOCH = Date.now();

/** The live Run tabs, by panel id. */
const tabs = new Map<string, RunTab>();
// The desired shimmer state per panel id, so a tab that mounts after
// its panel started loading still picks the shimmer up; entries die
// with their tab.
const loadingByPanel = new Map<string, boolean>();

/**
 * Drives one Run tab's shimmer. Called by the RunPanel on every state
 * transition; a tab that has not mounted yet picks the state up at
 * init, and one already gone is a no-op.
 */
export function setRunTabLoading(panelId: string, loading: boolean): void {
  loadingByPanel.set(panelId, loading);
  tabs.get(panelId)?.setLoading(loading);
}

export class RunTab extends Disposable implements ITabRenderer {
  public readonly element = document.createElement("div");
  private readonly content = document.createElement("div");
  private readonly close = document.createElement("button");
  private panelId: string | null = null;
  private loading = false;

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
    this.panelId = parameters.api.id;
    tabs.set(parameters.api.id, this);
    // The panel may have entered loading before the tab mounted.
    this.loading = loadingByPanel.get(parameters.api.id) ?? false;
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
    this.applyLoading();
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
