// The shared model-selection state. App-aware and DOM-free: the
// composition root constructs one instance and hands it to the Agent
// controller and the Model menu through their constructors; consumers
// read the state directly or subscribe to the change events.
//
// The server owns the selection. setCurrent is a command: it sends a
// select_model event through the injected send function and mutates
// nothing. State changes only when the server's workbench snapshot
// arrives - whoever handles the socket's onWorkbench calls
// applySelected(frame.selected), which never sends, so a snapshot can
// never echo back onto the wire.

import { Emitter, type Event } from "../base/event";
import { Disposable } from "../base/lifecycle";
import type { CatalogModel } from "./protocol";

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

  /**
   * @param sendSelect Puts one select_model event on the wire (the
   * composition root injects WorkshopSocket.selectModel); returns false
   * when nothing was sent so the caller can surface the failure.
   */
  constructor(private readonly sendSelect: (id: string) => boolean) {
    super();
  }

  /** The model catalog, as fetched at boot or pushed by the server. */
  get models(): readonly CatalogModel[] {
    return this._models;
  }

  /** The selected model's id, or "" when no model is selected. */
  get current(): string {
    return this._current;
  }

  /**
   * Records a catalog. The selection is untouched: the server owns it and
   * reconciles it against the new catalog in its next workbench snapshot.
   */
  setModels(entries: readonly CatalogModel[]): void {
    this._models = entries;
    this._onDidChangeModels.fire(entries);
  }

  /**
   * Asks the server to select `id`. A command, not a mutation: the
   * selection changes only when the workbench snapshot confirming it
   * arrives through applySelected. Returns false when the send failed.
   */
  setCurrent(id: string): boolean {
    return this.sendSelect(id);
  }

  /**
   * Applies the server-owned selection from a workbench snapshot; null
   * means no model is selected. Fires onDidChangeCurrent only on a real
   * change, and never sends, so applying a snapshot cannot echo.
   */
  applySelected(id: string | null): void {
    const next = id ?? "";
    if (next === this._current) {
      return;
    }
    this._current = next;
    this._onDidChangeCurrent.fire(next);
  }
}
