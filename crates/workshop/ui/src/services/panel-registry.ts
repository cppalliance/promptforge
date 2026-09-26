// The panel registry: the SPA-side mirror of the server's
// workshop-registry. Every dockview panel kind is declared here once -
// its zone affinity, default title, tab renderer, and the import thunk
// that lazy-loads its feature directory - and the feature's own
// register() installs the panel factory when the chunk resolves. The
// registry holds metadata eagerly and code lazily: zones.ts resolves
// placement from the entries below without the panel implementations
// (CodeMirror, Shiki, TipTap) landing in the initial bundle, and the
// first activation of a panel loads its directory on demand.
//
// The registry itself is DOM-free data plus the load machinery; the
// renderer that swaps a resolved panel into the dock lives in
// parts/layout/panel-types.ts.

import type { DockviewApi, IContentRenderer } from "dockview";

import { DisposableStore, type IDisposable } from "../base/lifecycle";
import { createServiceToken, type ServiceToken } from "./service-registry";
import type { ZoneName } from "./zone-state-service";

/** The registered name of the close-button-free tab renderer. */
export const PERMANENT_TAB = "permanent";
/** The registered name of the agent tab renderer with an SPA context menu. */
export const AGENT_TAB = "agent-tab";
/** The registered name of the Run window's shimmering-title tab renderer. */
export const RUN_TAB = "run-tab";

/** The panel kinds the workbench knows. */
export type PanelType = "tree" | "editor" | "config" | "agent" | "run";

/**
 * The contract a lazy feature directory's barrel (index.ts) satisfies.
 * register() installs what belongs to the chunk: the directory's panel
 * factory, chunk-bound quick-access providers, and chunk-sourced
 * context keys (the editor's activeEditor/editorLangId, which follow
 * the dock). Actions, menu rows, and keybinding rules do not belong
 * here - they live in the feature's eager <feature>.contribution.ts,
 * registered into the shared registries at module scope through
 * registerAction, because register() runs only when the chunk loads
 * and the boot-time menus must render before that. register() returns
 * a disposable the registry holds for the page lifetime. It may return
 * a promise when activation has async setup; the panel mounts after it
 * resolves.
 */
export interface PanelFeatureModule {
  register?: () => IDisposable | Promise<IDisposable> | void | Promise<void>;
}

/** One panel kind's registration: static metadata plus the lazy thunk. */
export interface PanelTypeEntry {
  readonly type: string;
  /** The zone a new panel opens in when the user has not moved it. */
  readonly defaultZone: ZoneName;
  readonly title: string;
  /** The named tab renderer, or undefined for Dockview's default tab. */
  readonly tabComponent: string | undefined;
  /** Loads the feature directory's barrel; esbuild splits it into a chunk. */
  readonly load: () => Promise<PanelFeatureModule>;
}

/** The composition root's dock, registered by initZones at boot. */
export const DOCK: ServiceToken<DockviewApi> = createServiceToken<DockviewApi>("workshop.dock");

/**
 * The seam a lazy panel wrapper exposes: the real panel once the feature
 * chunk has resolved. Panels that inspect dock content (the editor
 * commands' instanceof checks, the tree's focus command) must unwrap
 * through resolvePanelContent, never read view.content directly.
 */
export interface LazyPanelHost {
  readonly resolvedPanel: IContentRenderer | null;
}

/**
 * Unwraps a dockview content renderer to the real panel: a lazy wrapper
 * answers its resolved panel (itself while the chunk is still loading,
 * so instanceof checks against feature classes simply fail until then);
 * any other renderer answers itself.
 */
export function resolvePanelContent(content: IContentRenderer): IContentRenderer {
  const host = content as IContentRenderer & Partial<LazyPanelHost>;
  return host.resolvedPanel ?? content;
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
 * Declares one panel kind. Re-registering a type replaces its entry.
 * The returned disposable removes the registration.
 */
export function registerPanelType(entry: PanelTypeEntry): IDisposable {
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

/** The installed factory for a panel kind, once its chunk has loaded. */
export function panelFactory(type: string): (() => IContentRenderer) | undefined {
  return factories.get(type);
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

// The built-in panel kinds. The metadata is eager; the code behind each
// thunk is a lazy chunk. Heavy panels (the agent session's Shiki and
// TipTap, the editor's CodeMirror) leave the initial bundle this way.
registerPanelType({
  type: "tree",
  defaultZone: "left",
  title: "Workshop",
  // The Workshop tree anchors the workbench; its tab has no close
  // button, so the panel cannot be dismissed from the tab strip.
  tabComponent: PERMANENT_TAB,
  load: () => import("../parts/workspace/index"),
});
registerPanelType({
  type: "editor",
  defaultZone: "main",
  title: "Editor",
  tabComponent: undefined,
  load: () => import("../parts/editor/index"),
});
registerPanelType({
  type: "config",
  defaultZone: "main",
  title: "Gateway Config",
  tabComponent: undefined,
  load: () => import("../parts/gateway/index"),
});
registerPanelType({
  type: "agent",
  defaultZone: "right",
  title: "Agent Session",
  tabComponent: AGENT_TAB,
  load: () => import("../parts/agent/index"),
});
registerPanelType({
  type: "run",
  defaultZone: "main",
  title: "Run",
  tabComponent: RUN_TAB,
  load: () => import("../parts/run/index"),
});
