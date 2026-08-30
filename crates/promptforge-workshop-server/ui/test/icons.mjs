// Unit test for the lucide-backed icon strings (src/chat/utils/icons.ts).
// Bundles the module with esbuild, imports it via a data URL under jsdom
// (lucide's createElement needs a document at module load), and asserts
// every exported icon is a parseable inline SVG string carrying the
// dimensions and stroke attributes the old hand-pasted constants had -
// the vendored consumers assign these strings to innerHTML and their CSS
// sizes against the width/height attributes.
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
  entryPoints: [path.join(uiDir, "..", "src", "chat", "utils", "icons.ts")],
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

// Every export the vendored consumers import, with the pixel size the old
// hand-pasted string carried.
const expectedSizes = {
  ICON_COPY: 15,
  ICON_CHECK: 15,
  ICON_EDIT: 15,
  ICON_SETTINGS: 20,
  ICON_PAPERCLIP: 20,
  ICON_CHEVRON: 14,
  ICON_FORK: 15,
  ICON_MORE_HORIZONTAL: 16,
  ICON_MORE_VERTICAL: 16,
  ICON_PIN: 15,
  ICON_PIN_OFF: 15,
  ICON_TRASH: 15,
};

check(
  "the module exports exactly the twelve icon names the consumers import",
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

  check(`${name} keeps the old paste's width of ${size}`, svg.getAttribute("width") === String(size));
  check(`${name} keeps the old paste's height of ${size}`, svg.getAttribute("height") === String(size));
  check(`${name} keeps the 24-unit lucide viewBox`, svg.getAttribute("viewBox") === "0 0 24 24");
  check(`${name} is an outline icon with no fill`, svg.getAttribute("fill") === "none");
  check(`${name} keeps stroke-width 2`, svg.getAttribute("stroke-width") === "2");
  check(`${name} contains at least one drawing element`, svg.children.length > 0);

  const expectedStroke = name === "ICON_CHECK" ? "var(--mur-success)" : "currentColor";
  check(`${name} strokes with ${expectedStroke}`, svg.getAttribute("stroke") === expectedStroke);
}

if (failures.length > 0) {
  console.error(`icons: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("icons: all assertions passed");
