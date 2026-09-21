// The tree-state service: the Workshop file tree's expanded directory
// paths and the listings already fetched from /workspace/tree. The
// expansion belongs to the workspace: it seeds from the workspace
// bucket's "tree" value at construction and writes the same shape,
// `{ "expanded": [...] }`, back through a debounced writer on every
// change, so the folders the user left open come back on relaunch. The
// listing cache is session-only - a restored folder has no listing until
// the tree panel fetches one on render. The granted-roots listing is the
// exception: roots() loads it here, once, so the tree panel and the
// window title share a single GET /workspace/tree per boot and per
// workspace change instead of each fetching their own.
//
// The initial value arrives as unknown and passes a hand-written shape
// check - a malformed or hostile payload reads as nothing expanded, never
// as a cast. The writer is fire-and-forget: a write that throws is
// swallowed and the in-memory set stays authoritative.
//
// The service self-registers with a default factory (nothing expanded,
// no-op writer), so any bundle that touches it gets a working singleton;
// the composition root re-registers it bound to the live adapter before
// the first consumer resolves it.
//
// Generic and DOM-free: the initial value and the writer are injected,
// and nothing here may import from the app layers.

import { Emitter } from "../base/event";
import type { Event } from "../base/event";
import type { IDisposable } from "../base/lifecycle";
import { createServiceToken, registerService } from "./service-registry";
import { fetchTree, type TreeListing } from "./workspace-api";

/** Cache key for the synthetic granted-roots listing, which has no path. */
export const ROOTS_KEY = "";

/** The roots fetch roots() runs; tests inject a counting one. */
export type RootsFetch = (path: null) => Promise<TreeListing>;

// Matches the layout saver: a click-through of several folders lands as
// one write.
const SAVE_DEBOUNCE_MS = 250;

/** The writer the service hands `{ expanded: [...] }` to after a change. */
export type TreeStateWriter = (value: unknown) => void;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Narrows a path list: non-string and empty entries drop out, duplicates
 * collapse. Shared by the persisted shape and replaceExpanded.
 */
function readPaths(value: unknown): Set<string> {
  const paths = new Set<string>();
  if (!Array.isArray(value)) {
    return paths;
  }
  for (const item of value as readonly unknown[]) {
    if (typeof item === "string" && item !== "") {
      paths.add(item);
    }
  }
  return paths;
}

/**
 * Narrows the persisted payload to the expanded set: it must be an object
 * with an `expanded` array. Anything else reads as nothing expanded.
 */
function readExpanded(initial: unknown): Set<string> {
  return isRecord(initial) ? readPaths(initial.expanded) : new Set();
}

/**
 * The Workshop tree's expansion and listing state. The synthetic
 * granted-roots listing has no path; it caches under the empty-string
 * key, roots() loads it once for every consumer (a second caller during
 * the load joins the same promise), and invalidateRoots drops exactly
 * that entry, plus any load in flight, when the workspace grants change.
 */
export class TreeStateService implements IDisposable {
  private expanded: Set<string>;
  private readonly listingCache = new Map<string, TreeListing>();
  // The roots load consumers are waiting on, while the cache is empty.
  private rootsInFlight: Promise<TreeListing> | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private readonly changeEmitter = new Emitter<void>();

  /**
   * Fires when the whole expanded set is replaced (a workspace switch);
   * the tree panel re-renders from it. Interactive expand and collapse
   * do not fire: the panel that made the change already painted it.
   */
  readonly onDidChange: Event<void> = this.changeEmitter.event;

  /**
   * `initial` is the value the workspace bucket held at boot (any shape;
   * see readExpanded); `write` receives `{ expanded: [...] }` after each
   * change, debounced.
   */
  constructor(
    initial: unknown = null,
    private readonly write: TreeStateWriter = () => {},
  ) {
    this.expanded = readExpanded(initial);
  }

  /** The live expanded set, read-only; Save As snapshots it. */
  get expandedPaths(): ReadonlySet<string> {
    return this.expanded;
  }

  /** Whether a directory row renders expanded. */
  isExpanded(path: string): boolean {
    return this.expanded.has(path);
  }

  /** Marks a directory expanded. */
  expand(path: string): void {
    if (this.expanded.has(path)) {
      return;
    }
    this.expanded.add(path);
    this.scheduleWrite();
  }

  /** Marks a directory collapsed. */
  collapse(path: string): void {
    if (!this.expanded.delete(path)) {
      return;
    }
    this.scheduleWrite();
  }

  /**
   * Replaces the expanded set wholesale with a workspace's stored value
   * (Open Workspace from File) and fires onDidChange. Writes nothing: the
   * paths came from the file, and a write armed by an earlier interactive
   * change is cancelled so the previous workspace's set never lands in
   * the newly opened file.
   */
  replaceExpanded(paths: readonly string[]): void {
    this.cancelWrite();
    this.expanded = readPaths(paths);
    this.changeEmitter.fire();
  }

  /** The cached listing for a path, if one was fetched this session. */
  listing(path: string): TreeListing | undefined {
    return this.listingCache.get(path);
  }

  /** Every listing fetched this session; the quick-open file provider reads them. */
  cachedListings(): readonly TreeListing[] {
    return [...this.listingCache.values()];
  }

  /** Caches one fetched listing. */
  cacheListing(path: string, listing: TreeListing): void {
    this.listingCache.set(path, listing);
  }

  /**
   * The granted roots: the cached listing when present, else the load
   * already in flight, else a new one. Every consumer that needs the roots
   * (the tree panel, the window title) reads them here so a boot or a
   * workspace change costs one GET /workspace/tree. A successful load
   * caches only if no invalidateRoots ran while it was in flight; a stale
   * load still resolves for its own caller but never lands in the cache.
   * A rejected load clears the slot so the next call retries. `fetch`
   * defaults to the workspace API; tests inject a counting one.
   */
  roots(fetch: RootsFetch = fetchTree): Promise<TreeListing> {
    const cached = this.listingCache.get(ROOTS_KEY);
    if (cached !== undefined) {
      return Promise.resolve(cached);
    }
    if (this.rootsInFlight !== null) {
      return this.rootsInFlight;
    }
    const load: Promise<TreeListing> = fetch(null).then(
      (listing) => {
        if (this.rootsInFlight === load) {
          this.listingCache.set(ROOTS_KEY, listing);
          this.rootsInFlight = null;
        }
        return listing;
      },
      (error: unknown) => {
        if (this.rootsInFlight === load) {
          this.rootsInFlight = null;
        }
        throw error;
      },
    );
    this.rootsInFlight = load;
    return load;
  }

  /**
   * Drops the synthetic roots listing after the workspace grants changed
   * (a drop, an Add/Remove Folder, an Open), so the next roots() fetches
   * again. A load in flight is dropped too: it started against the old
   * grants, so it neither caches nor answers later callers. Directory
   * listings survive: the folders themselves did not change.
   */
  invalidateRoots(): void {
    this.listingCache.delete(ROOTS_KEY);
    this.rootsInFlight = null;
  }

  private scheduleWrite(): void {
    this.cancelWrite();
    this.timer = setTimeout(() => {
      this.timer = null;
      try {
        this.write({ expanded: [...this.expanded] });
      } catch {
        // The adapter reports its own failures; a throwing writer leaves
        // the in-memory set authoritative.
      }
    }, SAVE_DEBOUNCE_MS);
  }

  private cancelWrite(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  /** Drops the state and any armed write; a disposed service persists nothing. */
  dispose(): void {
    this.cancelWrite();
    this.expanded = new Set();
    this.listingCache.clear();
    this.rootsInFlight = null;
    this.changeEmitter.dispose();
  }
}

/** The registry token for the tree-state singleton. */
export const TREE_STATE = createServiceToken<TreeStateService>("workshop.treeState");

// Self-registration with nothing expanded and a no-op writer: a consumer
// that resolves the token before the composition root re-registers it
// bound to the live adapter gets a working, unpersisted tree state rather
// than a wrong instance cached for the page lifetime.
registerService(TREE_STATE, () => new TreeStateService());
