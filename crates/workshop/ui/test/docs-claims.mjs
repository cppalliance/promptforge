// Guard for the workspace-debt removal (plan step 7): the repository's
// prose describes the shipped behavior. Each phrase below was once true
// and is now stale, so its return would mean a doc was reverted or a
// paragraph copied from history. Reads the files with node:fs rather
// than importing anything, so the check needs no bundle and runs in the
// plain SPA suite.
// Run: node --test test/docs-claims.mjs
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
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

// Guard for the Engine, Harness, and Host definitions in the root AGENTS.md.
// Coding sessions read the rulebooks (every AGENTS.md, every .cursor/rules
// file, and the crate doc of every lib.rs or main.rs that carries
// `## Invariants`) as instructions, so each one uses the three words one way.
// Inline and fenced code is skipped, and so is the Definitions section itself,
// whose rule text names the words it restricts.
const SKIP_DIRS = new Set([".git", "node_modules", "target", "dist"]);
// Network and outside-tool phrases that keep a lowercase "host".
const HOST_ALLOWED = /host-and-address|self-hosted|github-hosted/gi;
// Other senses that keep a lowercase "engine" or "harness".
const TERM_ALLOWED = /speech engine|database engine|test harness/gi;
const RETIRED = ["production host", "engine's host", "hosts the engine", "harness that hosts", "host globals", "host-support"];

async function* walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) {
      yield path.join(dir, entry.name);
    } else if (!SKIP_DIRS.has(entry.name)) {
      yield* walk(path.join(dir, entry.name));
    }
  }
}

/** Every prose line of every rulebook, with inline code blanked, as `{ file, number, text }`. */
async function rulebookLines() {
  const out = [];
  for await (const full of walk(repoRoot)) {
    const file = path.relative(repoRoot, full).split(path.sep).join("/");
    const base = path.basename(file);
    let lines;
    if (base === "AGENTS.md" || (file.startsWith(".cursor/rules/") && base.endsWith(".mdc"))) {
      lines = (await readFile(full, "utf8")).split("\n").map((text, index) => ({ text, number: index + 1 }));
    } else if (base === "lib.rs" || base === "main.rs") {
      const doc = (await readFile(full, "utf8"))
        .split("\n")
        .map((text, index) => ({ text, number: index + 1 }))
        .filter(({ text }) => text.trimStart().startsWith("//!"));
      if (!doc.some(({ text }) => text.includes("## Invariants"))) {
        continue;
      }
      lines = doc.map(({ text, number }) => ({ text: text.trimStart().slice(3), number }));
    } else {
      continue;
    }
    let fenced = false;
    let definitions = false;
    for (const { text, number } of lines) {
      if (text.trimStart().startsWith("```")) {
        fenced = !fenced;
        continue;
      }
      if (file === "AGENTS.md" && text.startsWith("## ")) {
        definitions = text.startsWith("## Definitions");
      }
      if (!fenced && !definitions) {
        out.push({ file, number, text: text.replace(/`[^`]*`/g, " ") });
      }
    }
  }
  return out;
}

test("rulebooks use Engine, Harness, and Host as the root AGENTS.md defines them", async () => {
  const offenders = [];
  for (const { file, number, text } of await rulebookLines()) {
    const where = `${file}:${number}: ${text.trim()}`;
    for (const phrase of RETIRED) {
      if (text.toLowerCase().includes(phrase)) {
        offenders.push(`${where} (retired phrase "${phrase}")`);
      }
    }
    if (/\bhost(s|ed|ing)?\b|\bhost-/.test(text.replace(HOST_ALLOWED, " "))) {
      offenders.push(`${where} (lowercase "host": required "Host" for the application, another word for any other sense)`);
    }
    const terms = file.startsWith("crates/gateway/stt/") ? /(?<![-_/:\w])harness(?![-_/:\w])/ : /(?<![-_/:\w])(engine|harness)(?![-_/:\w])/;
    if (terms.test(text.replace(TERM_ALLOWED, " "))) {
      offenders.push(`${where} (lowercase "engine" or "harness": required "Engine" or "Harness" for the defined term)`);
    }
  }
  assert.deepEqual(offenders, [], "rulebook lines break the root AGENTS.md Definitions");
});
