// Shared fake for the UI-state adapter (src/services/ui-storage.ts): an
// in-memory UiStorage that every store test seeds with initial bucket
// values and inspects for the writes the store made. It mirrors the real
// adapter's contract - `get` answers null for an absent key, `set` updates
// the cache and records the call, `suppressWrites` drops workspace-bucket
// writes for the duration of the callback - without any fetch, so a store
// under test never touches the network. `replaceWorkspace` stands in for
// what the real adapter's `reloadWorkspace` does after Open: it swaps the
// workspace bucket for another file's values without recording a write.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.

/**
 * Builds a fake adapter. `initial` is `{ user: {...}, workspace: {...} }`;
 * either bucket may be omitted. The returned object has the adapter
 * surface plus `sets`, the recorded `set` calls in order as
 * `{ bucket, key, value }`, and `suppressed`, the workspace writes that
 * `suppressWrites` swallowed.
 */
export function createFakeUiStorage(initial = {}) {
  const buckets = {
    user: new Map(Object.entries(initial.user ?? {})),
    workspace: new Map(Object.entries(initial.workspace ?? {})),
  };
  const sets = [];
  const suppressed = [];
  let suppressDepth = 0;
  return {
    sets,
    suppressed,
    preload() {
      return Promise.resolve();
    },
    get(bucket, key) {
      const value = buckets[bucket].get(key);
      return value === undefined ? null : value;
    },
    set(bucket, key, value) {
      if (bucket === "workspace" && suppressDepth > 0) {
        suppressed.push({ bucket, key, value });
        return;
      }
      buckets[bucket].set(key, value);
      sets.push({ bucket, key, value });
    },
    reloadWorkspace() {
      return Promise.resolve();
    },
    replaceWorkspace(values) {
      buckets.workspace = new Map(Object.entries(values ?? {}));
    },
    suppressWrites(fn) {
      suppressDepth += 1;
      try {
        return fn();
      } finally {
        suppressDepth -= 1;
      }
    },
  };
}
