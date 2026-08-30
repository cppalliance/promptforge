// The one definition site of the config-ui layer rule: views may import
// components and services, components may import services, and services
// import only services - so services/ never reaches views/ or
// components/, and components/ never reaches views/. main.ts is the
// composition root: it may import every layer, and nothing may import
// it. Adapted from the workshop UI's check-layers.mjs; run by the
// package.json pretest script so npm test enforces the rule.
// Dependency-free on purpose: it must run before node_modules exists.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const defaultSrcDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "src");

const LAYERS = new Set(["services", "components", "views"]);

// The layer a file belongs to, from its path relative to srcDir:
// "main" for the composition root, a directory layer otherwise, null
// for shared root files (router.ts, css.d.ts) and anything outside.
function layerOf(filePath, srcDir) {
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
function describe(filePath, srcDir) {
  return path.relative(path.dirname(srcDir), filePath).split(path.sep).join("/");
}

/**
 * Checks one resolved import against the layer rule. The paths are
 * absolute; package (non-relative) imports are exempt and never reach
 * this function. Returns null when the import is allowed, or a
 * human-readable violation string.
 */
export function checkImport(importerPath, resolvedImportPath, srcDir = defaultSrcDir) {
  const importer = layerOf(importerPath, srcDir);
  const imported = layerOf(resolvedImportPath, srcDir);
  const from = describe(importerPath, srcDir);
  const to = describe(resolvedImportPath, srcDir);
  if (imported === "main") {
    return `${from} imports ${to}: nothing may import the composition root`;
  }
  if (importer === "services" && (imported === "views" || imported === "components")) {
    return `${from} imports ${to}: services may not import views or components`;
  }
  if (importer === "components" && imported === "views") {
    return `${from} imports ${to}: components may not import views`;
  }
  return null;
}

// Matches the specifier of `import ... from`, `export ... from`, bare
// `import "..."`, and dynamic `import("...")`. Only relative specifiers
// are checked, so a stray match on ordinary prose is harmless.
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

/**
 * The standalone walk: every .ts under `srcDir`, every relative import
 * resolved and checked. Returns the violation strings.
 */
export function runWalk(srcDir = defaultSrcDir) {
  const violations = [];
  for (const file of walk(srcDir)) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(IMPORT_RE)) {
      const specifier = match[1];
      if (!specifier.startsWith(".")) {
        continue;
      }
      const violation = checkImport(
        file,
        path.resolve(path.dirname(file), specifier),
        srcDir,
      );
      if (violation !== null) {
        violations.push(violation);
      }
    }
  }
  return violations;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const violations = runWalk(process.argv[2] ? path.resolve(process.argv[2]) : defaultSrcDir);
  if (violations.length > 0) {
    console.error(`check-layers: ${violations.length} layer violation(s):`);
    for (const violation of violations) {
      console.error(`  ${violation}`);
    }
    process.exit(1);
  }
  console.log("check-layers: ok");
}
