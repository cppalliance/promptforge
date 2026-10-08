// The look boundary guard: `@workshop/look` is the family's base visual
// layer, so its sources import only their own files. Walks
// every `.ts` and `.css` file outside `test/` and `node_modules/` and
// fails on any import, re-export, dynamic import, triple-slash reference,
// or CSS `@import` whose specifier is not a relative path that stays
// inside the package, naming the offending file and
// specifier. `shared-ui`, the Workshop UI, other `@workshop/*` packages,
// `lucide`, and escaping relative paths all fail. Type-only imports count: the scan
// reads source text, so nothing is elided before it looks. No jsdom: this
// is a source-text check.
// Run: node test/boundary.mjs (from crates/workshop/look).
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const TS_PATTERNS = [
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

// @import "a"; @import url("a"); @import url(a);
const CSS_IMPORT = /@import\s+(?:url\(\s*)?["']?([^"')\s;]+)/g;

export function importSpecifiers(file, text) {
  if (file.endsWith(".css")) {
    const code = text.replace(/\/\*[\s\S]*?\*\//g, "");
    return [...code.matchAll(CSS_IMPORT)].map((match) => match[1]);
  }
  return TS_PATTERNS.flatMap((pattern) => [...text.matchAll(pattern)].map((match) => match[1]));
}

export function violation(file, specifier) {
  if (!specifier.startsWith("./") && !specifier.startsWith("../")) {
    return "not a relative path";
  }
  const relative = path.relative(lookDir, path.resolve(path.dirname(file), specifier));
  if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    return "a relative path that leaves the package";
  }
  return null;
}

const files = (await readdir(lookDir, { recursive: true, withFileTypes: true }))
  .filter((entry) => entry.isFile() && /\.(ts|css)$/.test(entry.name))
  .map((entry) => path.join(entry.parentPath ?? entry.path, entry.name))
  .filter((file) => {
    const segments = path.relative(lookDir, file).split(path.sep);
    return segments[0] !== "test" && !segments.includes("node_modules");
  })
  .sort();

test("the walk reaches the package's TypeScript and CSS sources", () => {
  assert.ok(files.some((file) => file.endsWith(".ts")), "the walk found no .ts source");
  assert.ok(files.some((file) => file.endsWith(".css")), "the walk found no .css source");
});

test("the scan reads every import form", () => {
  const ts = [
    'import "./a.css";',
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
    'export type Kind = "success" | "error";',
    'export const k = Array.from("./not-an-import");',
  ].join("\n");
  assert.deepEqual(
    importSpecifiers(path.join(lookDir, "probe.ts"), ts).sort(),
    ["./a.css", "./b", "./c", "./e", "./f", "./g", "./h", "./i", "./j.d.ts"],
  );
  const css = [
    '@import "./a.css";',
    "@import url('./b.css');",
    "@import url(./c.css);",
    '/* @import "./commented.css"; */',
  ].join("\n");
  assert.deepEqual(importSpecifiers(path.join(lookDir, "probe.css"), css), ["./a.css", "./b.css", "./c.css"]);
});

test("the classifier admits own files and rejects everything else, lucide included", () => {
  const from = path.join(lookDir, "toast.ts");
  for (const specifier of ["./toast.css", "./icons", "../look/modal"]) {
    assert.equal(violation(from, specifier), null, `${specifier} should be allowed`);
  }
  for (const specifier of [
    "lucide",
    "@vscode/codicons",
    "shared-ui/toast",
    "@workshop/ui",
    "@workshop/shell",
    "lucide/dist/esm/icons/mic",
    "../ui/src/main",
    "../../shared-ui/tokens.css",
    "/abs/path",
  ]) {
    assert.notEqual(violation(from, specifier), null, `${specifier} should be rejected`);
  }
});

test("no look source imports outside the package", async () => {
  const offenders = [];
  for (const file of files) {
    for (const specifier of importSpecifiers(file, await readFile(file, "utf8"))) {
      const reason = violation(file, specifier);
      if (reason !== null) offenders.push(`${path.relative(lookDir, file)}: "${specifier}" (${reason})`);
    }
  }
  assert.deepEqual(offenders, [], `boundary violations:\n  ${offenders.join("\n  ")}`);
});
