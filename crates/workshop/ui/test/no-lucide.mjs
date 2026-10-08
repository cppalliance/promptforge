// The Workshop UI draws every icon from @workshop/look's codicon strings, so
// nothing under src/ imports lucide and the package no longer depends on it.
// (The Gateway config app keeps its own lucide icons; it is a separate
// package.) A source-text check: no jsdom.
// Run: node --test test/no-lucide.mjs
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = path.join(uiDir, "src");

const files = (await readdir(srcDir, { recursive: true, withFileTypes: true }))
  .filter((entry) => entry.isFile() && /\.(ts|css)$/.test(entry.name))
  .map((entry) => path.join(entry.parentPath ?? entry.path, entry.name))
  .sort();

test("the walk reaches the UI's sources", () => {
  assert.ok(files.length > 100, `only ${files.length} source files found; the walk is broken`);
});

test("no file under src/ mentions lucide", async () => {
  const offenders = [];
  for (const file of files) {
    const text = await readFile(file, "utf8");
    if (/lucide/i.test(text)) {
      offenders.push(path.relative(uiDir, file));
    }
  }
  assert.deepEqual(offenders, [], `lucide still named in:\n  ${offenders.join("\n  ")}`);
});

test("the package declares no lucide dependency", async () => {
  const manifest = JSON.parse(await readFile(path.join(uiDir, "package.json"), "utf8"));
  const declared = { ...manifest.dependencies, ...manifest.devDependencies };
  assert.equal("lucide" in declared, false);
});
