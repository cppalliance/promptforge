// The fork guard: the Workshop UI takes its visual layer from
// `@workshop/look`, never from `crates/shared-ui`, the Gateway's copy.
// Walks every file under `src/` and `test/` and fails on any line that
// contains `shared-ui`, naming the file and line. This file is skipped:
// it has to spell the name it forbids. No jsdom: this is a source-text
// check.
// Run: node test/no-shared-ui.mjs
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const self = fileURLToPath(import.meta.url);
const uiDir = path.join(path.dirname(self), "..");
const FORBIDDEN = "shared-ui";

async function filesUnder(dir) {
  return (await readdir(dir, { recursive: true, withFileTypes: true }))
    .filter((entry) => entry.isFile())
    .map((entry) => path.join(entry.parentPath ?? entry.path, entry.name));
}

const files = [
  ...(await filesUnder(path.join(uiDir, "src"))),
  ...(await filesUnder(path.join(uiDir, "test"))),
]
  .filter((file) => file !== self)
  .sort();

test("the walk reaches both src/ and test/", () => {
  const relative = files.map((file) => path.relative(uiDir, file).split(path.sep)[0]);
  assert.ok(relative.includes("src"), "the walk found no file under src/");
  assert.ok(relative.includes("test"), "the walk found no file under test/ besides this guard");
});

test("no file under src/ or test/ names shared-ui", async () => {
  const offenders = [];
  for (const file of files) {
    const lines = (await readFile(file, "utf8")).split("\n");
    lines.forEach((line, index) => {
      if (line.includes(FORBIDDEN)) {
        offenders.push(`${path.relative(uiDir, file)}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  assert.deepEqual(offenders, [], `the Workshop UI names ${FORBIDDEN}:\n  ${offenders.join("\n  ")}`);
});
