// Guard for the workspace-debt removal (plan step 7): the repository's
// prose describes the shipped behavior. Each phrase below was once true
// and is now stale, so its return would mean a doc was reverted or a
// paragraph copied from history. Reads the files with node:fs rather
// than importing anything, so the check needs no bundle and runs in the
// plain SPA suite.
// Run: node --test test/docs-claims.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

/** Every line of `file` (relative to the repo root) matching `phrase`, tagged with its number. */
async function offendingLines(file, phrase) {
  const text = await readFile(path.join(repoRoot, file), "utf8");
  return text
    .split("\n")
    .map((line, index) => ({ line, number: index + 1 }))
    .filter(({ line }) => line.includes(phrase))
    .map(({ line, number }) => `${file}:${number}: ${line.trim()}`);
}

test("AGENTS.md names two UI-state homes, not a TOML config or three buckets", async () => {
  // UM-002: the SPA rule once routed "account preferences" to a
  // "machine-written TOML config" that no code, route, or allow-list
  // implements, and counted three buckets where two exist.
  for (const phrase of ["TOML config", "three named buckets", "three homes"]) {
    assert.deepEqual(await offendingLines("AGENTS.md", phrase), [], `AGENTS.md still says "${phrase}"`);
  }
});
