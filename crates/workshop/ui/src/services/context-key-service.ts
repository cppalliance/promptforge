// The context-key service: the single store of named context values
// (editorTextFocus, editorLangId, inputFocus, config.editor.*) that
// menus, keybindings, and action preconditions evaluate their `when`
// expressions against. Features bind keys with createKey in their
// register() or in main.ts - never at contribution-module scope, because
// a service instance must exist first - and run bodies read or write
// them through the registry at call time. One global service holds every
// key; the seam for per-DOM-subtree scoping later is this service, not
// its callers.
//
// The change event includes affectsSome so a subscriber (a menu, the
// dispatcher) re-evaluates only the expressions that read the keys that
// actually changed, instead of every expression on every write.
//
// The service self-registers with a default factory, so any bundle that
// touches it gets the singleton without composition-root wiring.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { Emitter } from "../base/event";
import type { Event } from "../base/event";
import type { IDisposable } from "../base/lifecycle";
import { ContextKeyExpr } from "./context-key-expr";
import type { ContextKeyExpression } from "./context-key-expr";
import { createServiceToken, registerService } from "./service-registry";

/**
 * The change payload: affectsSome answers whether any of the subscriber's
 * referenced keys changed, so it can skip re-evaluation when they did not.
 */
export interface ContextKeyChangeEvent {
  affectsSome(keys: ReadonlySet<string>): boolean;
}

/**
 * A context key bound to a name in the service. set writes and fires the
 * change event; get returns the current value (the declared default until
 * the first set); reset restores the declared default and fires.
 */
export interface ContextKey<T> {
  set(value: T): void;
  get(): T;
  reset(): void;
}

/** The store of context values and the evaluator for when-expressions. */
export class ContextKeyService implements IDisposable {
  private readonly values = new Map<string, unknown>();
  private readonly changeEmitter = new Emitter<ContextKeyChangeEvent>();

  /** Fires when any key's value changes; layout and menu code hook it. */
  readonly onDidChangeContext: Event<ContextKeyChangeEvent> = this.changeEmitter.event;

  /**
   * Binds a key to `name` with `defaultValue`. The default is visible
   * through getValue immediately, so an expression can match before any
   * set. Re-binding an existing name keeps the current value.
   */
  createKey<T>(name: string, defaultValue: T): ContextKey<T> {
    if (!this.values.has(name)) {
      this.values.set(name, defaultValue);
    }
    return {
      set: (value: T): void => this.setValue(name, value),
      // The map holds unknown; the key's own writes are the only source
      // for this name after binding, so the read is the declared T.
      get: (): T => (this.values.get(name) as T | undefined) ?? defaultValue,
      reset: (): void => this.setValue(name, defaultValue),
    };
  }

  /** The current value of a key, or undefined when no key declared it. */
  getValue(name: string): unknown {
    return this.values.get(name);
  }

  /**
   * Evaluates a when-expression against the current values. Accepts a
   * parsed expression or a source string; undefined means "always". A
   * malformed string never matches - registration is where malformed
   * strings are reported, so a stray string reaching here is a bug that
   * should hide the row, not throw.
   */
  contextMatchesRules(expr: string | ContextKeyExpression | undefined): boolean {
    if (expr === undefined) {
      return true;
    }
    if (typeof expr === "string") {
      const result = ContextKeyExpr.deserialize(expr);
      if (!result.ok) {
        return false;
      }
      return result.value.evaluate((key) => this.getValue(key));
    }
    return expr.evaluate((key) => this.getValue(key));
  }

  private setValue(name: string, value: unknown): void {
    this.values.set(name, value);
    const changed: ReadonlySet<string> = new Set([name]);
    this.changeEmitter.fire({
      affectsSome: (keys) => {
        for (const key of keys) {
          if (changed.has(key)) {
            return true;
          }
        }
        return false;
      },
    });
  }

  dispose(): void {
    this.changeEmitter.dispose();
  }
}

/** The registry token for the context-key singleton. */
export const CONTEXT_KEY_SERVICE = createServiceToken<ContextKeyService>("workshop.contextKey");

// Self-registration: the default instance is shared by every consumer in
// the process. The composition root may re-register to rebind.
registerService(CONTEXT_KEY_SERVICE, () => new ContextKeyService());
