// The shared model-selection state, formerly module-level mutables in
// main.ts. App-aware and DOM-free: the composition root constructs one
// instance and hands it to the Agent controller and the Model menu
// through their constructors; consumers read the state directly or
// subscribe to the change events.

import { Emitter, type Event } from "../base/event";
import { Disposable } from "../base/lifecycle";
import type { CatalogModel } from "../workshop-socket";

/**
 * Owns the model catalog and the current model selection shared by every
 * Agent tab and the title-bar Model menu.
 */
export class ModelService extends Disposable {
  private _models: readonly CatalogModel[] = [];
  private _current = "";

  private readonly _onDidChangeModels = this._register(
    new Emitter<readonly CatalogModel[]>(),
  );
  /** Fires with the new catalog after every setModels call. */
  readonly onDidChangeModels: Event<readonly CatalogModel[]> = this._onDidChangeModels.event;

  private readonly _onDidChangeCurrent = this._register(new Emitter<string>());
  /** Fires with the new id whenever the current selection changes. */
  readonly onDidChangeCurrent: Event<string> = this._onDidChangeCurrent.event;

  /** The model catalog, as fetched at boot or pushed by the server. */
  get models(): readonly CatalogModel[] {
    return this._models;
  }

  /** The selected model's id, or "" when no model is selected. */
  get current(): string {
    return this._current;
  }

  /**
   * Records a catalog, keeping the current selection when it survives
   * the refresh and falling back to the first entry (or none) when it
   * does not.
   */
  setModels(entries: readonly CatalogModel[]): void {
    this._models = entries;
    this._onDidChangeModels.fire(entries);
    if (!entries.some((entry) => entry.id === this._current)) {
      this.setCurrent(entries[0]?.id ?? "");
    }
  }

  /** Selects `id`; re-selecting the current model notifies nobody. */
  setCurrent(id: string): void {
    if (id === this._current) {
      return;
    }
    this._current = id;
    this._onDidChangeCurrent.fire(id);
  }
}
