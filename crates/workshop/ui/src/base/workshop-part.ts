// The WorkshopPart base class: the root of the workbench's Part
// hierarchy (the VS Code Part/Composite pattern). Every dockview panel
// extends it. The base owns the panel's root element, runs the subclass's
// create() exactly once on the first init - dockview calls init when the
// panel mounts, and a restored or re-added panel must never rebuild its
// DOM - and provides the layout() hook panels override when they care
// about their dimensions. Disposal comes from Disposable: every child a
// panel registers through _register tears down with one dispose() from
// the dock.
//
// Generic panel infrastructure: nothing here may import from the feature
// directories.

import type { GroupPanelPartInitParameters, IContentRenderer } from "dockview";

import { Disposable } from "./lifecycle";

/** A width/height pair, as the dock reports it to resizable parts. */
export interface IDimension {
  readonly width: number;
  readonly height: number;
}

export abstract class WorkshopPart extends Disposable implements IContentRenderer {
  readonly element: HTMLElement = document.createElement("div");
  private created = false;

  /**
   * Dockview's mount seam: builds the part's content into its element on
   * the first call. Later calls (a panel re-added after a layout restore)
   * leave the built DOM alone.
   */
  init(_parameters: GroupPanelPartInitParameters): void {
    if (this.created) {
      return;
    }
    this.created = true;
    this.create(this.element);
  }

  /** Builds the part's content under `parent`. Called once, by init. */
  protected abstract create(parent: HTMLElement): void;

  /**
   * Reacts to a resize. A no-op for parts that lay themselves out. The
   * dual signature serves both contracts: dockview calls parts with
   * (width, height); workshop code passes an IDimension, matching the
   * VS Code Part hierarchy this class models.
   */
  layout(dimension: IDimension): void;
  layout(width: number, height: number): void;
  layout(..._args: [IDimension] | [number, number]): void {}
}
