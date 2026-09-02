// Unit test for the lucide-backed icon strings (src/ui/workshop/icons.ts).
// Bundles the module with esbuild, imports it via a data URL under jsdom
// (lucide's createElement needs a document at module load), and asserts
// every exported icon is a parseable inline SVG string carrying the
// dimensions and stroke attributes the tree panel's CSS sizes against -
// the panel assigns these strings to innerHTML.
// Run: node test/icons.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;

const result = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "ui", "workshop", "icons.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const code = result.outputFiles[0].text;
const icons = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Every export the workbench panels import, with its pixel size.
const expectedSizes = {
  ICON_TRASH_2: 15,
  ICON_FOLDER_PLUS: 15,
  ICON_MIC: 16,
  ICON_SEND: 16,
};

check(
  "the module exports exactly the icon names the panels import",
  Object.keys(icons).sort().join(",") === Object.keys(expectedSizes).sort().join(","),
);

const host = dom.window.document.createElement("div");
for (const [name, size] of Object.entries(expectedSizes)) {
  const value = icons[name];
  check(`${name} is a non-empty string`, typeof value === "string" && value.length > 0);
  if (typeof value !== "string") continue;

  host.innerHTML = value;
  const svg = host.firstElementChild;
  check(`${name} parses to a single svg element`, host.children.length === 1 && svg?.tagName.toLowerCase() === "svg");
  if (!svg || svg.tagName.toLowerCase() !== "svg") continue;

  check(`${name} keeps its width of ${size}`, svg.getAttribute("width") === String(size));
  check(`${name} keeps its height of ${size}`, svg.getAttribute("height") === String(size));
  check(`${name} keeps the 24-unit lucide viewBox`, svg.getAttribute("viewBox") === "0 0 24 24");
  check(`${name} is an outline icon with no fill`, svg.getAttribute("fill") === "none");
  check(`${name} keeps stroke-width 2`, svg.getAttribute("stroke-width") === "2");
  check(`${name} contains at least one drawing element`, svg.children.length > 0);
  check(`${name} strokes with currentColor`, svg.getAttribute("stroke") === "currentColor");
}

if (failures.length > 0) {
  console.error(`icons: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("icons: all assertions passed");
