// The WorkshopPart base class: the root of the workbench's Part
// hierarchy (the VS Code Part/Composite pattern). Every dockview panel
// extends it. The base owns the panel's root element, keeps the panel api
// dockview hands each init, runs the subclass's create() exactly once on
// the first init - dockview calls init when the panel mounts, and a
// restored or re-added panel must never rebuild its DOM - and provides the
// layout() hook panels override when they care about their dimensions and
// the confirmClose() veto the close commands await. Disposal comes from
// Disposable: every child a panel registers through _register tears down
// with one dispose() from the dock.
//
// Generic panel infrastructure: imports only this package's own files and
// type-only `dockview`.

import type { DockviewPanelApi, GroupPanelPartInitParameters, IContentRenderer } from "dockview";

import { Disposable } from "./lifecycle";

/** A width/height pair, as the dock reports it to resizable parts. */
export interface IDimension {
  readonly width: number;
  readonly height: number;
}

export abstract class WorkshopPart extends Disposable implements IContentRenderer {
  readonly element: HTMLElement = document.createElement("div");
  /** The dock's handle on this panel, from the latest init; null before the first. */
  protected panelApi: DockviewPanelApi | null = null;
  private created = false;

  /**
   * Dockview's mount seam: stores the panel api, then builds the part's
   * content into its element on the first call. Later calls (a panel
   * re-added after a layout restore) leave the built DOM alone.
   */
  init(parameters: GroupPanelPartInitParameters): void {
    this.panelApi = parameters.api;
    if (this.created) {
      return;
    }
    this.created = true;
    this.create(this.element);
  }

  /** Builds the part's content under `parent`. Called once, by init. */
  protected abstract create(parent: HTMLElement): void;

  /**
   * Whether the part may close; the close commands await the answer before
   * closing its panel. Resolves true. A part holding unsaved work
   * overrides it to ask first.
   */
  confirmClose(): Promise<boolean> {
    return Promise.resolve(true);
  }

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
