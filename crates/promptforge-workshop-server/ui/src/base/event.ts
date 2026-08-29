// Minimal typed event primitive for the workshop UI. Generic and DOM-free:
// nothing here may import from the app layers.

import type { IDisposable } from "./lifecycle";

/** Subscribes `listener`; disposing the returned handle unsubscribes it. */
export type Event<T> = (listener: (value: T) => void) => IDisposable;

/**
 * Single-signal typed emitter. The owner keeps the Emitter private, exposes
 * `event` for subscribers, publishes with fire(), and routes the whole thing
 * through its lifecycle so dispose() severs every subscription at once.
 */
export class Emitter<T> implements IDisposable {
  private readonly _listeners = new Set<(value: T) => void>();
  private _isDisposed = false;

  /** The subscribe function handed to consumers (e.g. `onDidChange`). */
  readonly event: Event<T> = (listener) => {
    if (!this._isDisposed) {
      this._listeners.add(listener);
    }
    return { dispose: () => this._listeners.delete(listener) };
  };

  /** Delivers `value` to every currently subscribed listener. */
  fire(value: T): void {
    if (this._isDisposed) {
      return;
    }
    for (const listener of this._listeners) {
      listener(value);
    }
  }

  /** Drops all listeners; later fire() and event() calls are no-ops. */
  dispose(): void {
    this._isDisposed = true;
    this._listeners.clear();
  }
}
