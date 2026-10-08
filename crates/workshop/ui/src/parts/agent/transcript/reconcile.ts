// The keyed reconcile the transcript uses at every level (turns, a turn's
// rows, a group's steps). Given the next list of keyed entries it reuses
// the component for a key, creates the ones that are new, removes the ones
// that are gone, and puts the elements in order. A component is reused
// only while its `kind` still matches - a lone thought that becomes a
// group keeps its key but is a different component - so a kind change
// replaces the component. The replaced component stays alive until the
// new one is built and has been offered to `carry`, so a caller can hand
// over what the operator chose (an open row) before the old one is
// disposed.
//
// Elements that live after the managed ones (a turn's footer, the tail
// status) are not touched: new components land before the first
// unmanaged child, so those extras stay where the caller put them.

import type { IDisposable } from "@workshop/platform/lifecycle";

/** A component the reconcile manages. */
export interface KeyedComponent extends IDisposable {
  readonly kind: string;
  readonly element: HTMLElement;
}

/** The slice of an entry the reconcile reads. */
export interface Keyed {
  readonly key: string;
  readonly kind: string;
}

/**
 * Reconciles `parent`'s managed children against `next`. `components` is
 * the caller's key-to-component map for this parent, updated in place.
 * `carry`, when given, runs for a component that replaced one of another
 * kind under the same key - after the new one's first `update`, before
 * the old one is disposed.
 * Returns the components in order.
 */
export function reconcile<T extends Keyed, C extends KeyedComponent>(
  parent: HTMLElement,
  components: Map<string, C>,
  next: readonly T[],
  create: (entry: T) => C,
  update: (component: C, entry: T) => void,
  carry?: (component: C, replaced: C, entry: T) => void,
): C[] {
  const nextByKey = new Map(next.map((entry) => [entry.key, entry]));
  const replaced = new Map<string, C>();
  for (const [key, component] of [...components]) {
    const entry = nextByKey.get(key);
    if (entry?.kind === component.kind) {
      continue;
    }
    component.element.remove();
    components.delete(key);
    if (entry === undefined) {
      component.dispose();
    } else {
      replaced.set(key, component);
    }
  }

  const ordered: C[] = [];
  let previous: HTMLElement | null = null;
  for (const entry of next) {
    let component = components.get(entry.key);
    let carried: C | undefined;
    if (component === undefined) {
      component = create(entry);
      components.set(entry.key, component);
      carried = replaced.get(entry.key);
    }
    update(component, entry);
    if (carried !== undefined) {
      carry?.(component, carried, entry);
    }
    const element = component.element;
    const expected: ChildNode | null = previous === null ? parent.firstChild : previous.nextSibling;
    if (element !== expected) {
      parent.insertBefore(element, expected);
    }
    previous = element;
    ordered.push(component);
  }
  for (const component of replaced.values()) {
    component.dispose();
  }
  return ordered;
}
