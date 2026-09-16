// The service registry: one module-level collection mapping service
// tokens to lazy factories, the SPA-side mirror of the server's
// workshop-registry proxy slots. Subsystems self-register at module scope
// or from the composition root; consumers resolve with getService and
// never name each other's modules. Instantiation is lazy - the factory
// runs on the first get, and the instance is cached for the page
// lifetime, which is what lets session state (zone placement, the tree's
// expansion) survive the panels that own it closing and reopening.
//
// Generic and DOM-free: nothing here may import from the app layers.

import { toDisposable, type IDisposable } from "../base/lifecycle";

/**
 * The identity of one service. The type parameter is phantom: it only
 * ties a token to the type its factory must produce, so a lookup cannot
 * resolve the wrong service without a type error.
 */
export interface ServiceToken<T> {
  readonly id: string;
  /** Phantom brand; never present at runtime. */
  readonly _service?: T;
}

/** Mints a service token. Ids are diagnostic only, but keep them unique. */
export function createServiceToken<T>(id: string): ServiceToken<T> {
  return { id };
}

interface Registration {
  readonly factory: () => unknown;
  /** The lazily built instance; undefined until the first get. */
  instance: unknown;
  /** True once the factory has run (instance may still be undefined). */
  built: boolean;
}

const registrations = new Map<ServiceToken<unknown>, Registration>();

/**
 * Receives one notification per getService call while installed via
 * setServiceObserver. Test-only seam: the boot test helper is its only
 * consumer, recording where the first resolution falls in the boot
 * sequence relative to the fetches around it.
 */
export interface IServiceObserver {
  /** Called on every resolution, before the factory may run. */
  serviceResolved(id: string): void;
}

let observer: IServiceObserver | undefined;

/**
 * Installs `next` as the resolution observer, or clears it with undefined.
 * Off by default, so production pays one undefined check per resolution.
 * Test-only seam: the shared boot test helper (test/helpers/boot.mjs) is
 * its only consumer.
 */
export function setServiceObserver(next: IServiceObserver | undefined): void {
  observer = next;
}

/**
 * Registers `factory` as the producer for `token`. Re-registering a token
 * replaces the factory and drops any cached instance - the next get
 * rebuilds from the new factory - which is how tests rebind a service
 * (the dock, a status sink) without process restarts. The returned
 * disposable unregisters only if this registration is still current, so
 * disposing a stale registration cannot evict its replacement.
 */
export function registerService<T>(token: ServiceToken<T>, factory: () => T): IDisposable {
  const registration: Registration = { factory: () => factory(), instance: undefined, built: false };
  registrations.set(token as ServiceToken<unknown>, registration);
  return toDisposable(() => {
    if (registrations.get(token as ServiceToken<unknown>) === registration) {
      registrations.delete(token as ServiceToken<unknown>);
    }
  });
}

/**
 * Resolves `token`, building the instance on first use. Throws when the
 * token was never registered: a missing service is a wiring bug, and a
 * loud throw naming the token beats a downstream null dereference.
 */
export function getService<T>(token: ServiceToken<T>): T {
  observer?.serviceResolved(token.id);
  const registration = registrations.get(token as ServiceToken<unknown>);
  if (registration === undefined) {
    throw new Error(`no service registered for ${token.id}`);
  }
  if (!registration.built) {
    registration.instance = registration.factory();
    registration.built = true;
  }
  return registration.instance as T;
}

/**
 * Resolves `token` when present, null when unregistered. For optional
 * composition-root services: a panel created outside the composition root
 * (a registry test) gets no status bar and stays silent rather than
 * failing to construct.
 */
export function getServiceOrNull<T>(token: ServiceToken<T>): T | null {
  return registrations.has(token as ServiceToken<unknown>) ? getService(token) : null;
}
