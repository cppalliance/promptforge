// The panel registry: every dockview panel kind is declared here once -
// its zone affinity, title, identity rule, and the import thunk that
// lazy-loads its feature directory - and the feature's own register()
// installs the panel factory when the chunk resolves. The registry is
// open: it declares no panel kind itself; each feature registers its own
// from an eager module that evaluates before the layout boots. It holds
// metadata eagerly and code lazily: placement resolves from the entries
// without the panel implementations landing in the initial bundle, and
// the first activation of a panel loads its directory on demand.
//
// The registry itself is DOM-free data plus the load machinery; the
// renderer that swaps a resolved panel into the dock lives with the
// application.

import type { DockviewApi, IContentRenderer } from "dockview";

import { DisposableStore, type IDisposable } from "./lifecycle";
import { createServiceToken, type ServiceToken } from "./service-registry";

/** The dock's named tab banks. */
export const ZONE_NAMES = ["left", "main", "right"] as const;
export type ZoneName = (typeof ZONE_NAMES)[number];

/** A registered panel kind's name. */
export type PanelType = string;

/**
 * Parameters passed into a panel open. They must be JSON-safe: Dockview
 * serializes them into the saved layout, and a restored panel receives
 * them back as its only state.
 */
export type PanelParams = Record<string, unknown>;

/**
 * The contract a lazy feature directory's barrel (index.ts) satisfies.
 * register() installs what belongs to the chunk: the directory's panel
 * factory, chunk-bound quick-access providers, and context keys that
 * follow the dock. Actions, menu rows, and keybinding rules do not belong
 * here - they register at module scope from an eager module, because
 * register() runs only when the chunk loads and the boot-time menus must
 * render before that. register() returns a disposable the registry holds
 * for the page lifetime. It may return a promise when activation has
 * async setup; the panel mounts after it resolves.
 */
export interface PanelFeatureModule {
  register?: () => IDisposable | Promise<IDisposable> | void | Promise<void>;
}

/** One panel kind's registration: static metadata plus the lazy thunk. */
export interface PanelTypeEntry {
  readonly type: PanelType;
  /** The tab title: fixed, or computed from the open's params. */
  readonly title: string | ((params: PanelParams) => string);
  /** The zone a new panel opens in when the user has not moved it. */
  readonly defaultZone: ZoneName;
  /** False when the tab strip must not close the panel. Defaults to true. */
  readonly closable?: boolean;
  /**
   * True when the operator may rename the panel's tab: a double-click opens
   * an inline editor and the tab menu offers Rename. The name lasts for the
   * session only; the panel's own title is untouched. Defaults to false.
   */
  readonly renamable?: boolean;
  /**
   * The panel id for one open. Without it, a string `instance` param keys
   * the id as `type:instance`, and otherwise the kind is a singleton.
   * An id must start with `type:` or equal `type`, so the type is
   * recoverable by splitting at the first colon.
   */
  readonly panelId?: (params: PanelParams) => string;
  /** Loads the feature directory's barrel; esbuild splits it into a chunk. */
  readonly load: () => Promise<PanelFeatureModule>;
}

/** The composition root's dock, registered by the zone layer at boot. */
export const DOCK: ServiceToken<DockviewApi> = createServiceToken<DockviewApi>("workshop.dock");

/**
 * The seam a lazy panel wrapper exposes: the real panel once the feature
 * chunk has resolved. Code that inspects dock content (instanceof checks
 * against a feature's panel class) must unwrap through
 * resolvePanelContent, never read view.content directly.
 */
export interface LazyPanelContainer {
  readonly resolvedPanel: IContentRenderer | null;
}

/**
 * Unwraps a dockview content renderer to the real panel: a lazy wrapper
 * answers its resolved panel (itself while the chunk is still loading,
 * so instanceof checks against feature classes simply fail until then);
 * any other renderer answers itself.
 */
export function resolvePanelContent(content: IContentRenderer): IContentRenderer {
  const container = content as IContentRenderer & Partial<LazyPanelContainer>;
  return container.resolvedPanel ?? content;
}

const entries = new Map<string, PanelTypeEntry>();
const factories = new Map<string, () => IContentRenderer>();
// The directories whose register() has already run, keyed by the module
// namespace object the thunk resolved to: two panel kinds sharing one
// directory still register it once.
const registeredModules = new WeakSet<object>();
const loadPromises = new Map<string, Promise<(() => IContentRenderer) | undefined>>();
// Owns every directory registration's disposable. Page-lifetime by
// design: a panel closing must not unregister its directory's commands
// and factories, because a reopened panel needs them again.
const registrationStore = new DisposableStore();

/**
 * Declares one panel kind. Throws when the type is already registered.
 * The returned disposable removes the registration.
 */
export function registerPanelType(entry: PanelTypeEntry): IDisposable {
  if (entries.has(entry.type)) {
    throw new Error(`the panel type "${entry.type}" is already registered`);
  }
  entries.set(entry.type, entry);
  loadPromises.delete(entry.type);
  return {
    dispose: () => {
      if (entries.get(entry.type) === entry) {
        entries.delete(entry.type);
        loadPromises.delete(entry.type);
      }
    },
  };
}

/**
 * Installs a panel kind's factory; the feature directory's register()
 * calls this when its chunk loads. Re-registering replaces the factory.
 */
export function registerPanelFactory(
  type: string,
  factory: () => IContentRenderer,
): IDisposable {
  factories.set(type, factory);
  return {
    dispose: () => {
      if (factories.get(type) === factory) {
        factories.delete(type);
      }
    },
  };
}

/** Narrows a Dockview component name to a registered panel type. */
export function isPanelType(name: string): name is PanelType {
  return entries.has(name);
}

/** The registration for a panel kind, or undefined for unknown names. */
export function panelTypeEntry(name: string): PanelTypeEntry | undefined {
  return entries.get(name);
}

/**
 * Loads a panel kind's feature chunk, running the directory's register()
 * the first time, and answers the installed factory. Concurrent loads of
 * one type share the in-flight promise. Undefined when the type is
 * unknown; rejects when the chunk loads but installs no factory.
 */
export function loadPanelType(type: string): Promise<(() => IContentRenderer) | undefined> {
  const existing = loadPromises.get(type);
  if (existing !== undefined) {
    return existing;
  }
  const entry = entries.get(type);
  if (entry === undefined) {
    return Promise.resolve(undefined);
  }
  const promise = (async () => {
    const module = await entry.load();
    if (!registeredModules.has(module)) {
      registeredModules.add(module);
      const registration = await module.register?.();
      if (registration !== undefined && registration !== null) {
        registrationStore.add(registration);
      }
    }
    const factory = factories.get(type);
    if (factory === undefined) {
      throw new Error(`the ${type} feature directory registered no panel factory`);
    }
    return factory;
  })();
  loadPromises.set(type, promise);
  // A failed load must not poison the registry: the next activation
  // retries the import.
  promise.catch(() => loadPromises.delete(type));
  return promise;
}
