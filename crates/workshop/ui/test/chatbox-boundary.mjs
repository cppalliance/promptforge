// The chat box boundary guard: the exit check from the chatbox
// extraction's Testing Plan. `src/parts/chatbox/` is an isolated
// component - it imports only `base/lifecycle`, `shared/icons`, skin
// tokens through CSS, `lucide`, and `@tiptap/*` - so this test walks
// every file in the directory and fails on any quoted import prefix that
// reaches back into the host layers (`"../agent`, `"../stt`,
// `"../chrome`, `"../../services`; bare quoted prefixes, so an
// `import type` line trips it too) or on the string `grant`, the host
// concern that must never leak into the component. No jsdom: this is a
// source-text check.
// Run: node --test test/chatbox-boundary.mjs
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const chatboxDir = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "src",
  "parts",
  "chatbox",
);

// Spelled as joined fragments so this guard's own source never matches
// its own rule were it ever moved beside the component.
const FORBIDDEN = [
  { name: "an import from parts/agent", pattern: '"' + "../agent" },
  { name: "an import from parts/stt", pattern: '"' + "../stt" },
  { name: "an import from parts/chrome", pattern: '"' + "../chrome" },
  { name: "an import from services/", pattern: '"' + "../../services" },
  { name: "the host's access-control vocabulary", pattern: ["gr", "ant"].join("") },
];

const files = (await readdir(chatboxDir, { recursive: true, withFileTypes: true }))
  .filter((entry) => entry.isFile())
  .map((entry) => path.join(entry.parentPath ?? entry.path, entry.name))
  .sort();

test("the chatbox directory holds the component's files", () => {
  assert.ok(files.length > 0, "src/parts/chatbox/ holds no files; the walk is broken");
});

test("no file under src/parts/chatbox/ reaches into the host layers", async () => {
  const offenders = [];
  for (const file of files) {
    const lines = (await readFile(file, "utf8")).split("\n");
    lines.forEach((line, index) => {
      for (const { name, pattern } of FORBIDDEN) {
        if (line.includes(pattern)) {
          offenders.push(`${path.relative(chatboxDir, file)}:${index + 1} (${name}): ${line.trim()}`);
        }
      }
    });
  }
  assert.deepEqual(offenders, [], `boundary violations:\n  ${offenders.join("\n  ")}`);
});
