// The built bundle's test-only seams. The entry bundle exports nothing
// (main.ts is an entry point) and esbuild tree-shakes the unused test-only
// setters (setDisposableTracker, setServiceObserver) away, so the seams
// are unreachable from outside the bundle. attachSeams reattaches them by
// appending exports to the bundle text: the dist bytes execute unmodified,
// and each appended function assigns the bundle's own module-scope
// variable, located by its single distinctive call site. Consumers import
// the returned chunk paths through file URLs, because the split bundle's
// relative chunk imports cannot resolve from a data: URL.
// Export-only module: the node --test runner discovers every file under
// test/, so running this file directly must (and does) exit 0.
import { readdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const distDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "dist");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Each seam is a module-scope variable the built code reads through one
// distinctive call site (minified identifiers change per build, property
// names do not; identifiers may contain $, which \w excludes). The
// appended export named `name` assigns that variable.
const SEAMS = [
  // src/base/lifecycle.ts: the DisposableStore constructor's tracker call.
  { name: "__setDisposableTracker", callSite: /([\w$]+)\?\.trackCreated\(this\)/ },
  // src/services/service-registry.ts: getService's observer call.
  { name: "__setServiceObserver", callSite: /([\w$]+)\?\.serviceResolved\([\w$]+\.id\)/ },
];

/**
 * Scans every dist script for the SEAMS call sites and appends the missing
 * exports, answering a map from seam name to the path of the chunk that
 * carries it. With code splitting a seam may live in any chunk, and two
 * seams may share one. Idempotent: dist is not rebuilt between test runs,
 * so a previous run's appended exports may already be there.
 */
export async function attachSeams() {
  const found = {};
  const distScripts = (await readdir(distDir, { recursive: true }))
    .filter((name) => name.endsWith(".js"))
    .map((name) => path.join(distDir, name));
  for (const scriptPath of distScripts) {
    const source = await readFile(scriptPath, "utf8");
    const exports = [];
    for (const seam of SEAMS) {
      const match = source.match(seam.callSite);
      if (!match) continue;
      found[seam.name] = scriptPath;
      if (!source.includes(seam.name)) {
        exports.push(`export function ${seam.name}(next) { ${match[1]} = next; }`);
      }
    }
    if (exports.length > 0) {
      await appendAtomically(scriptPath, source, exports);
    }
    if (Object.keys(found).length === SEAMS.length) break;
  }
  const missing = SEAMS.map((seam) => seam.name).filter((name) => !(name in found));
  if (missing.length > 0) {
    throw new Error(
      `bundle-seams.mjs could not locate ${missing.join(", ")} in dist/; rebuild dist or retune the seam regex`,
    );
  }
  return found;
}

// Atomic append: node --test runs the boot tests concurrently, and a
// truncate-and-write would let a concurrent scanner read a partial chunk -
// missing the seam entirely, or worse, appending the export to truncated
// bytes and corrupting the chunk. Write a per-process temp file and rename
// it over the chunk so readers only ever see complete content.
async function appendAtomically(scriptPath, source, exports) {
  const tempPath = `${scriptPath}.${process.pid}.tmp`;
  await writeFile(tempPath, `${source}\n${exports.join("\n")}\n`);
  // Windows: when two boot tests lose the scan race together, both rename
  // over the chunk, and the loser's rename fails with EPERM while the
  // winner's freshly replaced file is still held open. The append is
  // idempotent, so a chunk that already carries every export satisfies
  // every concurrent appender; only a chunk still missing one after the
  // retries is a real failure.
  const names = SEAMS.map((seam) => seam.name).filter((name) => exports.some((e) => e.includes(name)));
  for (let attempt = 0; ; attempt++) {
    try {
      await rename(tempPath, scriptPath);
      return;
    } catch (error) {
      const current = await readFile(scriptPath, "utf8").catch(() => "");
      if (names.every((name) => current.includes(name))) {
        await rm(tempPath, { force: true });
        return;
      }
      if (error?.code !== "EPERM" || attempt >= 4) {
        await rm(tempPath, { force: true });
        throw error;
      }
      await sleep(50);
    }
  }
}
