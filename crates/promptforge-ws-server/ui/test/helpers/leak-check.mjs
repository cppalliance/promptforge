// Shared leak check for workshop UI tests: assertNoLeaks(lifecycle, run)
// fails when `run` creates DisposableStores it never disposes. Each test
// bundles its own copy of src/base/lifecycle.ts, so the caller hands in
// that bundled module and the tracker lands on the exact class instance
// the code under test uses. This helper is the only consumer of the
// setDisposableTracker seam in lifecycle.ts.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.
// Covered by: test/leak-check.mjs.

/**
 * Runs `run` (sync or async) with DisposableStore tracking enabled on
 * `lifecycle` - a loaded copy of src/base/lifecycle.ts exposing
 * setDisposableTracker - and throws if any store created during the run
 * is still undisposed afterwards, naming each leak by its construction
 * site. The tracker is uninstalled on the way out even when `run` throws.
 */
export async function assertNoLeaks(lifecycle, run) {
  const live = new Map();
  lifecycle.setDisposableTracker({
    trackCreated(store) {
      live.set(store, new Error().stack ?? "(stack unavailable)");
    },
    trackDisposed(store) {
      live.delete(store);
    },
  });
  try {
    await run();
  } finally {
    lifecycle.setDisposableTracker(undefined);
  }
  if (live.size > 0) {
    const sites = [...live.values()].map(
      (stack, index) => `  leak ${index + 1}: created at ${constructionSite(stack)}`,
    );
    throw new Error(`${live.size} DisposableStore(s) leaked:\n${sites.join("\n")}`);
  }
}

// Keeps the frames that name the leak: the tracker's own frame and the
// DisposableStore constructor are noise, and three frames are enough to
// see which component was constructed where.
function constructionSite(stack) {
  const frames = stack
    .split("\n")
    .slice(1)
    .map((line) => line.trim())
    .filter(
      (line) => !line.includes("trackCreated") && !line.includes("new DisposableStore"),
    );
  return frames.slice(0, 3).join("\n    ") || "(construction site unknown)";
}
