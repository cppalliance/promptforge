// The platform boundary guard: `@workshop/platform` is the family's
// browser-side UI mechanics, so its sources import only their own files
// and `dockview`. Walks every `.ts` file outside `test/` and
// `node_modules/` and fails on any import, re-export, dynamic import, or
// triple-slash reference whose specifier is neither a relative path that
// stays inside the package nor `dockview`, naming the offending file and
// specifier. `@workshop/look`, the Workshop UI, other packages, and
// escaping relative paths all fail. Type-only imports count: the scan
// reads source text, so nothing is elided before it looks. No jsdom: this
// is a source-text check.
// Run: node --test test/boundary.mjs (from crates/workshop/platform).
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const platformDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const PATTERNS = [
  // import x from "a"; import { x } from "a"; import type { X } from "a";
  // export { x } from "a"; export * from "a". The clause holds no quotes,
  // so the match cannot run on into a string literal.
  /^\s*(?:import|export)\b[^;"'`]*?\bfrom\s*["']([^"']+)["']/gm,
  // import "a";
  /^\s*import\s*["']([^"']+)["']/gm,
  // import("a")
  /\bimport\s*\(\s*["']([^"']+)["']/g,
  // import x = require("a")
  /\brequire\s*\(\s*["']([^"']+)["']/g,
  // /// <reference path="a" /> and /// <reference types="a" />
  /^\s*\/\/\/\s*<reference\s+(?:path|types)\s*=\s*["']([^"']+)["']/gm,
];

export function importSpecifiers(text) {
  return PATTERNS.flatMap((pattern) => [...text.matchAll(pattern)].map((match) => match[1]));
}

export function violation(file, specifier) {
  if (specifier === "dockview") return null;
  if (!specifier.startsWith("./") && !specifier.startsWith("../")) {
    return "not a relative path or dockview";
  }
  const relative = path.relative(platformDir, path.resolve(path.dirname(file), specifier));
  if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    return "a relative path that leaves the package";
  }
  return null;
}

const files = (await readdir(platformDir, { recursive: true, withFileTypes: true }))
  .filter((entry) => entry.isFile() && entry.name.endsWith(".ts"))
  .map((entry) => path.join(entry.parentPath ?? entry.path, entry.name))
  .filter((file) => {
    const segments = path.relative(platformDir, file).split(path.sep);
    return segments[0] !== "test" && !segments.includes("node_modules");
  })
  .sort();

test("the walk reaches the package's TypeScript sources", () => {
  assert.ok(files.length > 0, "the walk found no .ts source");
});

test("the scan reads every import form", () => {
  const ts = [
    'import "./a";',
    'import x from "./b";',
    "import {",
    "  c,",
    "  d,",
    '} from "./c";',
    'import type { E } from "./e";',
    'export { f } from "./f";',
    'export * from "./g";',
    'const h = await import("./h");',
    'import i = require("./i");',
    '/// <reference path="./j.d.ts" />',
    'export type Kind = "ok" | "err";',
    'export const k = Array.from("./not-an-import");',
  ].join("\n");
  assert.deepEqual(importSpecifiers(ts).sort(), ["./a", "./b", "./c", "./e", "./f", "./g", "./h", "./i", "./j.d.ts"]);
});

test("the classifier admits own files and dockview and rejects everything else", () => {
  const from = path.join(platformDir, "result.ts");
  for (const specifier of ["./result", "../platform/result", "dockview"]) {
    assert.equal(violation(from, specifier), null, `${specifier} should be allowed`);
  }
  for (const specifier of [
    "@workshop/look",
    "@workshop/look/icons",
    "workshop-ui",
    "@workshop/ui",
    "dockview-core",
    "dockview/dist/esm",
    "lucide",
    "../ui/src/services/error-catalog",
    "../look/icons",
    "/abs/path",
  ]) {
    assert.notEqual(violation(from, specifier), null, `${specifier} should be rejected`);
  }
});

test("no platform source imports outside the package or dockview", async () => {
  const offenders = [];
  for (const file of files) {
    for (const specifier of importSpecifiers(await readFile(file, "utf8"))) {
      const reason = violation(file, specifier);
      if (reason !== null) offenders.push(`${path.relative(platformDir, file)}: "${specifier}" (${reason})`);
    }
  }
  assert.deepEqual(offenders, [], `boundary violations:\n  ${offenders.join("\n  ")}`);
});
