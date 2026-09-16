// The tree-state service: the Workshop file tree's session state - the
// expanded directory paths and the listings already fetched from
// /workspace/tree. This state used to sit at module scope in
// workshop-panel.ts so a reopened panel would restore the tree as the
// user left it; as a registry-held service the same survival comes from
// the service registry's cached singleton instead of module internals.
// The service self-registers with a default factory, so any bundle that
// touches it gets the singleton without composition-root wiring.

import type { IDisposable } from "../base/lifecycle";
import { createServiceToken, registerService } from "./service-registry";
import type { TreeListing } from "./workspace-api";

/** Cache key for the synthetic granted-roots listing, which has no path. */
export const ROOTS_KEY = "";

/**
 * The Workshop tree's expansion and listing state. The synthetic
 * granted-roots listing has no path; it caches under the empty-string
 * key, and invalidateRoots drops exactly that entry when the workspace
 * grants change.
 */
export class TreeStateService implements IDisposable {
  private readonly expandedPaths = new Set<string>();
  private readonly listingCache = new Map<string, TreeListing>();

  /** Whether a directory row renders expanded. */
  isExpanded(path: string): boolean {
    return this.expandedPaths.has(path);
  }

  /** Marks a directory expanded. */
  expand(path: string): void {
    this.expandedPaths.add(path);
  }

  /** Marks a directory collapsed. */
  collapse(path: string): void {
    this.expandedPaths.delete(path);
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
   * Drops the synthetic roots listing after the workspace grants changed
   * (a drop or an Add/Remove Folder), so the next render refetches them.
   * Directory listings survive: the folders themselves did not change.
   */
  invalidateRoots(): void {
    this.listingCache.delete("");
  }

  dispose(): void {
    this.expandedPaths.clear();
    this.listingCache.clear();
  }
}

/** The registry token for the tree-state singleton. */
export const TREE_STATE = createServiceToken<TreeStateService>("workshop.treeState");

// Self-registration: the default instance is shared by every consumer in
// the process. The composition root may re-register to rebind.
registerService(TREE_STATE, () => new TreeStateService());
