// Reads the packaged dist/app.css and asserts the action menu's rules
// (@workshop/look/dropdown.css) landed in it: the vendored predecessor's
// stylesheet had silently dropped out of the bundle, leaving the menu
// unstyled. Whitespace-tolerant: the packaged bundle minifies to `.x{`
// while the debug build that `cargo build` writes emits `.x {`; both must
// include the rules.
// Run: node test/dropdown-bundle-css.mjs (after `npm run build`).
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "..", "dist");

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The bundled stylesheet's name is content-hashed; the build's manifest
// maps the logical name to it.
const manifest = JSON.parse(await readFile(path.join(distDir, "manifest.json"), "utf8"));
const appCss = await readFile(path.join(distDir, manifest["app.css"]), "utf8");
check("the bundled app.css includes the menu surface rules", /\.menu-item\s*\{/.test(appCss));
check("the bundled app.css includes the popup menu's rules", /\.menu-popup\s*\{/.test(appCss));

if (failures.length > 0) {
  console.error(`dropdown-bundle-css: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("dropdown-bundle-css: all assertions passed");
