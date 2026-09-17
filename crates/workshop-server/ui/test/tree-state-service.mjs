// Unit test for the tree-state service (src/services/tree-state-service.ts):
// the Workshop tree's expansion state and fetched listing cache, held as
// a registry service so a reopened panel restores the tree as the user
// left it, and - since the workspace-bucket move - seeded from the
// workspace file's "tree" value at construction and written back through
// a debounced writer so the expansion survives a relaunch. Bundles the
// module with esbuild and drives it against the shared fake UI-state
// adapter. Covers: self-registration through the TREE_STATE token with a
// working default; the singleton surviving across lookups; the initial
// expanded set applied from `{ expanded: [...] }` (a malformed value reads
// as empty); expansion tracking; a burst of expand/collapse coalescing
// into one writer call carrying the same shape; a throwing writer leaving
// the in-memory set intact; replaceExpanded replacing the set, firing
// onDidChange, cancelling a pending write, and writing nothing itself;
// the listing cache; root invalidation dropping only the roots listing;
// and roots(): concurrent callers share one fetch, a cached listing
// answers without one, an invalidation during an in-flight load makes
// the next call fetch again and keeps the stale result out of the cache,
// and a rejected fetch clears the slot so the next call retries.
// Run: node test/tree-state-service.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { TreeStateService, TREE_STATE } from "./src/services/tree-state-service.ts";
      export { getService } from "./src/services/service-registry.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { TreeStateService, TREE_STATE, getService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The writer debounces at 250 ms; this outwaits it.
const settle = () => new Promise((resolve) => setTimeout(resolve, 350));

/** A service bound to a fake adapter's workspace "tree" key. */
function bound(initial) {
  const storage = createFakeUiStorage({ workspace: initial === undefined ? {} : { tree: initial } });
  const service = new TreeStateService(storage.get("workspace", "tree"), (value) =>
    storage.set("workspace", "tree", value),
  );
  return { storage, service };
}

const SRC = "C:\\project\\src";
const DOCS = "C:\\project\\docs";

// --- Self-registration and the singleton ------------------------------------

{
  const service = getService(TREE_STATE);
  check("the TREE_STATE token self-registers", service instanceof TreeStateService);
  check("the registry caches one instance", getService(TREE_STATE) === service);
  check("the default instance starts with nothing expanded", service.expandedPaths.size === 0);
  // The default's writer is a no-op: expanding through it must not throw.
  service.expand(SRC);
  check("the default instance tracks expansion without a writer", service.isExpanded(SRC));
  service.dispose();
}

// --- The initial expanded set -----------------------------------------------

{
  const { service } = bound({ expanded: [SRC, DOCS, SRC, "", 42] });
  check("the initial expanded set is applied", service.isExpanded(SRC) && service.isExpanded(DOCS));
  check(
    "duplicates, empty strings, and non-strings drop out of the initial set",
    service.expandedPaths.size === 2,
  );
  check("expandedPaths exposes the live set", [...service.expandedPaths].join(",") === `${SRC},${DOCS}`);
  service.dispose();
}

for (const [label, malformed] of [
  ["null", null],
  ["a bare array", [SRC]],
  ["a string", "C:\\project"],
  ["an object without expanded", { paths: [SRC] }],
  ["expanded that is not an array", { expanded: SRC }],
]) {
  const { service } = bound(malformed);
  check(`${label} as the initial value reads as nothing expanded`, service.expandedPaths.size === 0);
  service.dispose();
}

// --- Expansion tracking -----------------------------------------------------

{
  const { service } = bound();
  check("a directory starts collapsed", !service.isExpanded(SRC));
  service.expand(SRC);
  check("expand marks the directory expanded", service.isExpanded(SRC));
  service.collapse(SRC);
  check("collapse marks the directory collapsed", !service.isExpanded(SRC));
  service.dispose();
}

// --- The debounced writer -----------------------------------------------------

{
  const { storage, service } = bound();
  service.expand(SRC);
  service.expand(DOCS);
  service.collapse(SRC);
  service.expand(SRC);
  check("no write lands synchronously", storage.sets.length === 0);
  await settle();
  check("a burst of expand and collapse coalesces into one writer call", storage.sets.length === 1);
  const written = storage.sets[0];
  check("the write targets the workspace bucket's tree key", written?.bucket === "workspace" && written?.key === "tree");
  check(
    "the written shape is { expanded: [...] } with the live set",
    JSON.stringify(written?.value) === JSON.stringify({ expanded: [DOCS, SRC] }),
  );

  // A second, separate change writes again.
  service.collapse(DOCS);
  await settle();
  check("a later change writes again", storage.sets.length === 2);
  check(
    "the second write carries the reduced set",
    JSON.stringify(storage.sets[1]?.value) === JSON.stringify({ expanded: [SRC] }),
  );

  // A no-op change (expanding what is already expanded) writes nothing.
  service.expand(SRC);
  await settle();
  check("a no-op expand writes nothing", storage.sets.length === 2);

  // Dispose cancels an armed write.
  service.expand(DOCS);
  service.dispose();
  await settle();
  check("dispose cancels a pending write", storage.sets.length === 2);
}

// --- A throwing writer leaves the set intact ---------------------------------

{
  let throws = 0;
  const service = new TreeStateService(null, () => {
    throws += 1;
    throw new Error("adapter down");
  });
  service.expand(SRC);
  await settle();
  check("the throwing writer was invoked", throws === 1);
  check("a rejected write leaves the in-memory set intact", service.isExpanded(SRC));
  service.dispose();
}

// --- replaceExpanded ------------------------------------------------------------

{
  const { storage, service } = bound({ expanded: [SRC] });
  let changes = 0;
  const sub = service.onDidChange(() => {
    changes += 1;
  });
  // Arm a pending write from a live change, then replace: the replace
  // must cancel it so the old workspace's set never lands in the new file.
  service.expand(DOCS);
  service.replaceExpanded(["C:\\other\\lib", "C:\\other\\lib", ""]);
  check("replaceExpanded replaces the set", !service.isExpanded(SRC) && !service.isExpanded(DOCS));
  check("replaceExpanded keeps the new paths, deduplicated", [...service.expandedPaths].join(",") === "C:\\other\\lib");
  check("replaceExpanded emits change", changes === 1);
  await settle();
  check("replaceExpanded writes nothing and cancels the pending write", storage.sets.length === 0);
  sub.dispose();
  service.dispose();
}

// --- The listing cache -------------------------------------------------------------

{
  const { service } = bound();
  const listing = { path: SRC, entries: [] };
  check("an uncached path has no listing", service.listing(SRC) === undefined);
  service.cacheListing(SRC, listing);
  check("the listing round-trips", service.listing(SRC) === listing);

  // The roots listing lives under the synthetic empty-path key; a workspace
  // change invalidates it without dropping the directory listings.
  service.cacheListing("", { path: null, entries: [] });
  check("the roots listing caches under the empty key", service.listing("") !== undefined);
  service.invalidateRoots();
  check("invalidateRoots drops the roots listing", service.listing("") === undefined);
  check("invalidateRoots keeps the directory listings", service.listing(SRC) === listing);
  service.dispose();
}

// --- roots(): one shared load ---------------------------------------------------

/** A gated roots fetch: counts calls, parks each on `gate` until released. */
function gatedFetch(listings) {
  let release = () => {};
  const gate = new Promise((resolve) => {
    release = () => resolve();
  });
  const calls = [];
  const fetch = async (path) => {
    calls.push(path);
    const next = listings[calls.length - 1];
    await gate;
    if (next instanceof Error) {
      throw next;
    }
    return next;
  };
  return { fetch, calls, release: () => release() };
}

{
  const { service } = bound();
  const roots = { path: null, entries: [] };
  const { fetch, calls, release } = gatedFetch([roots]);
  const first = service.roots(fetch);
  const second = service.roots(fetch);
  check("two concurrent roots() calls share one fetch", calls.length === 1);
  check("the shared fetch asks for the roots (path null)", calls[0] === null);
  check("two concurrent roots() calls receive the same promise", first === second);
  check("the roots are not cached while the load is in flight", service.listing("") === undefined);
  release();
  check("the shared load resolves both callers", (await first) === roots && (await second) === roots);
  check("a settled load caches the roots listing", service.listing("") === roots);
  check("a cached listing answers roots() without a fetch", (await service.roots(fetch)) === roots && calls.length === 1);
  service.dispose();
}

// --- roots(): invalidation during an in-flight load --------------------------

{
  const { service } = bound();
  const stale = { path: null, entries: [] };
  const fresh = { path: null, entries: [] };
  const staleFetch = gatedFetch([stale]);
  const freshFetch = gatedFetch([fresh]);
  const staleLoad = service.roots(staleFetch.fetch);
  service.invalidateRoots();
  const freshLoad = service.roots(freshFetch.fetch);
  check("invalidateRoots during an in-flight load makes the next roots() fetch again", freshFetch.calls.length === 1);
  check("the load after an invalidation is a new promise", staleLoad !== freshLoad);
  freshFetch.release();
  check("the fresh load resolves to the fresh listing", (await freshLoad) === fresh);
  check("the fresh listing is cached", service.listing("") === fresh);
  staleFetch.release();
  check("the stale load still resolves for its own caller", (await staleLoad) === stale);
  check("the stale load does not populate the cache", service.listing("") === fresh);
  check(
    "a later roots() reads the fresh cache without a fetch",
    (await service.roots(staleFetch.fetch)) === fresh && staleFetch.calls.length === 1,
  );
  service.dispose();
}

{
  // The stale load settles after the invalidation but before any new
  // call: it must leave the cache empty rather than resurrect the old roots.
  const { service } = bound();
  const stale = { path: null, entries: [] };
  const { fetch, release } = gatedFetch([stale]);
  const staleLoad = service.roots(fetch);
  service.invalidateRoots();
  release();
  await staleLoad;
  check("a stale load settling into an empty cache caches nothing", service.listing("") === undefined);
  service.dispose();
}

// --- roots(): a rejected fetch clears the slot ---------------------------------

{
  const { service } = bound();
  const roots = { path: null, entries: [] };
  const { fetch, calls, release } = gatedFetch([new Error("server down"), roots]);
  const failing = service.roots(fetch);
  release();
  let rejected = false;
  try {
    await failing;
  } catch (error) {
    rejected = error instanceof Error && error.message === "server down";
  }
  check("a failed roots load rejects its caller", rejected);
  check("a failed roots load caches nothing", service.listing("") === undefined);
  const retried = await service.roots(fetch);
  check("a rejected fetch clears the slot so the next roots() retries", calls.length === 2 && retried === roots);
  check("the retried load caches its listing", service.listing("") === roots);
  service.dispose();
}

// --- roots(): dispose drops an in-flight load ----------------------------------

{
  const { service } = bound();
  const roots = { path: null, entries: [] };
  const { fetch, release } = gatedFetch([roots]);
  const load = service.roots(fetch);
  service.dispose();
  release();
  await load;
  check("a load settling after dispose caches nothing", service.listing("") === undefined);
}

if (failures.length > 0) {
  console.error(`tree-state-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("tree-state-service: all assertions passed");
process.exit(0);
