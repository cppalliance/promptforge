// The one definition site of the UI layer rule: ui may import services may
// import base, never the reverse; main.ts is the composition root -
// it may import every layer, and nothing may import it. Consumed three ways
// so both build paths and CI enforce the same rule: build.mjs wraps
// checkImport in an esbuild onResolve plugin, build.rs spawns this file as a
// standalone walk before bundling (the esbuild CLI cannot load plugins), and
// the package.json typecheck script runs the same walk. Dependency-free on
// purpose: it must run even under the `npx esbuild` fallback, where
// node_modules may be absent.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const srcDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "src");

const LAYERS = new Set(["base", "services", "ui"]);

// The layer a file belongs to, from its path relative to src/: "main" for
// the composition root, a directory layer otherwise, null for a file
// outside src/ or in no layer.
function layerOf(filePath) {
  const relative = path.relative(srcDir, filePath);
  if (relative === "" || relative.startsWith("..") || path.isAbsolute(relative)) {
    return null;
  }
  const parts = relative.split(path.sep);
  if (parts.length === 1) {
    return parts[0] === "main.ts" || parts[0] === "main" ? "main" : null;
  }
  return LAYERS.has(parts[0]) ? parts[0] : null;
}

// A path as violation listings show it: relative to ui/, forward slashes.
function describe(filePath) {
  return path.relative(path.dirname(srcDir), filePath).split(path.sep).join("/");
}

/**
 * Checks one resolved import against the layer rule. Both arguments are
 * absolute paths; package (non-relative) imports are exempt and never reach
 * this function. Returns null when the import is allowed, or a
 * human-readable violation string.
 */
export function checkImport(importerPath, resolvedImportPath) {
  const importer = layerOf(importerPath);
  const imported = layerOf(resolvedImportPath);
  const from = describe(importerPath);
  const to = describe(resolvedImportPath);
  if (imported === "main") {
    return `${from} imports ${to}: nothing may import the composition root`;
  }
  if (importer === "main") {
    return null;
  }
  if (importer === null) {
    return `${from} sits in no layer (main.ts, base/, services/, or ui/)`;
  }
  if (imported === null) {
    return `${from} imports ${to}, which sits in no layer`;
  }
  if (importer === "base" && imported !== "base") {
    return `${from} imports ${to}: base may import only base`;
  }
  if (importer === "services" && imported === "ui") {
    return `${from} imports ${to}: services may import only base and services`;
  }
  return null;
}

// Matches the specifier of `import ... from`, `export ... from`, bare
// `import "..."`, and dynamic `import("...")`. Only relative specifiers are
// checked, so a stray match on ordinary prose is harmless.
const IMPORT_RE = /(?:\bfrom\s*|\bimport\s*\(?\s*)["']([^"']+)["']/g;

function walk(dir, files = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(full, files);
    } else if (entry.name.endsWith(".ts")) {
      files.push(full);
    }
  }
  return files;
}

// The standalone walk: every .ts under src/, every relative import
// resolved and checked.
function runWalk() {
  const violations = [];
  for (const file of walk(srcDir)) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(IMPORT_RE)) {
      const specifier = match[1];
      if (!specifier.startsWith(".")) {
        continue;
      }
      const violation = checkImport(file, path.resolve(path.dirname(file), specifier));
      if (violation !== null) {
        violations.push(violation);
      }
    }
  }
  return violations;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const violations = runWalk();
  if (violations.length > 0) {
    console.error(`check-layers: ${violations.length} layer violation(s):`);
    for (const violation of violations) {
      console.error(`  ${violation}`);
    }
    process.exit(1);
  }
  console.log("check-layers: ok");
}
