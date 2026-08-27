// Object lifetime primitives for the workshop UI. Generic and DOM-free:
// nothing here may import from the app layers.

/** A resource that frees itself when dispose() is called. */
export interface IDisposable {
  dispose(): void;
}

/**
 * Wraps a bare cleanup function as an IDisposable, so ad hoc teardown
 * (removeEventListener pairs, unsubscribe closures, timer clears) can be
 * routed through a DisposableStore.
 */
export function toDisposable(dispose: () => void): IDisposable {
  return { dispose };
}

/**
 * Receives DisposableStore lifetime notifications while installed via
 * setDisposableTracker. Test-only seam: the shared leak-check test helper
 * is its only consumer.
 */
export interface IDisposableTracker {
  /** Called when a store is constructed. */
  trackCreated(store: DisposableStore): void;
  /** Called the first time a store is disposed. */
  trackDisposed(store: DisposableStore): void;
}

let tracker: IDisposableTracker | undefined;

/**
 * Installs `next` as the store tracker, or clears it with undefined.
 * Off by default, so production pays one undefined check per store
 * construction and disposal. Test-only seam: the shared leak-check test
 * helper (test/helpers/leak-check.mjs) is its only consumer.
 */
export function setDisposableTracker(next: IDisposableTracker | undefined): void {
  tracker = next;
}

/** Collects disposables and releases them together, in insertion order. */
export class DisposableStore implements IDisposable {
  private readonly _items = new Set<IDisposable>();
  private _isDisposed = false;

  constructor() {
    tracker?.trackCreated(this);
  }

  /**
   * Takes ownership of `item` and returns it. If the store is already
   * disposed the item is disposed immediately, so late registration
   * cannot leak.
   */
  add<T extends IDisposable>(item: T): T {
    if (this._isDisposed) {
      item.dispose();
    } else {
      this._items.add(item);
    }
    return item;
  }

  /** Disposes everything held. Safe to call more than once. */
  dispose(): void {
    if (this._isDisposed) {
      return;
    }
    this._isDisposed = true;
    tracker?.trackDisposed(this);
    for (const item of this._items) {
      item.dispose();
    }
    this._items.clear();
  }
}

/**
 * Base class for objects that own other disposables. Subclasses route
 * every child (listener, emitter, nested Disposable) through _register
 * so that one dispose() tears down the whole tree.
 */
export abstract class Disposable implements IDisposable {
  private readonly _store = new DisposableStore();

  /** Ties `item`'s lifetime to this object's and returns it. */
  protected _register<T extends IDisposable>(item: T): T {
    return this._store.add(item);
  }

  /** Releases every registered child. */
  dispose(): void {
    this._store.dispose();
  }
}
