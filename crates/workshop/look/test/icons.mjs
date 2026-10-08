// Unit test for the codicon icon strings (icons.ts). Bundles the module with
// esbuild, imports it via a data URL under jsdom, and asserts that every
// exported icon is a parseable inline SVG string that equals its
// @vscode/codicons source file (src/icons/<name>.svg) apart from the size
// attributes, and that the four names the panels already import keep
// their pixel sizes - the panels assign these strings to innerHTML and
// their CSS sizes against the attributes. The package is a devDependency
// that only this test reads: the shipped strings are inline.
// Run: node test/icons.mjs (from crates/workshop/look).
import { readFile } from "node:fs/promises";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;

const result = await esbuild.build({
  entryPoints: [path.join(lookDir, "icons.ts")],
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

// Every export, with the codicon it renders and its pixel size. The four
// names the panels already import keep their sizes (15, 15, 16, 16) and
// switch to new-folder, trash, mic, and arrow-up; the rest are 16px.
const expected = {
  ICON_FOLDER_PLUS: { codicon: "new-folder", size: 15 },
  ICON_TRASH_2: { codicon: "trash", size: 15 },
  ICON_MIC: { codicon: "mic", size: 16 },
  ICON_SEND: { codicon: "arrow-up", size: 16 },
  ICON_CLOSE: { codicon: "close", size: 16 },
  ICON_CHEVRON_RIGHT: { codicon: "chevron-right", size: 16 },
  ICON_CHEVRON_DOWN: { codicon: "chevron-down", size: 16 },
  ICON_STOP_CIRCLE: { codicon: "stop-circle", size: 16 },
  ICON_ADD: { codicon: "add", size: 16 },
  ICON_ELLIPSIS: { codicon: "ellipsis", size: 16 },
  ICON_WARNING: { codicon: "warning", size: 16 },
  ICON_INFO: { codicon: "info", size: 16 },
  ICON_ERROR: { codicon: "error", size: 16 },
  ICON_CHECK: { codicon: "check", size: 16 },
  ICON_COPY: { codicon: "copy", size: 16 },
  // The glyphs the composer and agent chrome draw once lucide is gone: the five
  // mode glyphs and the mention chip's file, folder, and link family.
  ICON_AGENT: { codicon: "agent", size: 16 },
  ICON_CHECKLIST: { codicon: "checklist", size: 16 },
  ICON_BUG: { codicon: "bug", size: 16 },
  ICON_LAYERS: { codicon: "layers", size: 16 },
  ICON_ASK: { codicon: "ask", size: 16 },
  ICON_FILE: { codicon: "file", size: 16 },
  ICON_FILE_CODE: { codicon: "file-code", size: 16 },
  ICON_FILE_MEDIA: { codicon: "file-media", size: 16 },
  ICON_FILE_TEXT: { codicon: "file-text", size: 16 },
  ICON_FOLDER: { codicon: "folder", size: 16 },
  ICON_GLOBE: { codicon: "globe", size: 16 },
  ICON_LINK: { codicon: "link", size: 16 },
  ICON_SYMBOL_KEYWORD: { codicon: "symbol-keyword", size: 16 },
  // The title bar's toolbars: Toggle Primary Side Bar, Toggle Agents, and the settings gear.
  ICON_LAYOUT_SIDEBAR_LEFT: { codicon: "layout-sidebar-left", size: 16 },
  ICON_LAYOUT_SIDEBAR_RIGHT: { codicon: "layout-sidebar-right", size: 16 },
  ICON_GEAR: { codicon: "gear", size: 16 },
};

check(
  "the module exports exactly the icon names the surfaces import",
  Object.keys(icons).sort().join(",") === Object.keys(expected).sort().join(","),
);
check(
  "every export is a different codicon, thirty-one in all",
  new Set(Object.values(expected).map((entry) => entry.codicon)).size === 31,
);

// The codicon package root, found through its package.json.
const require = createRequire(path.join(lookDir, "package.json"));
const codiconDir = path.dirname(require.resolve("@vscode/codicons/package.json"));

// The size attributes are the only thing the strings may change.
const withoutSize = (svg) => svg.replace(/\s(?:width|height)="[^"]*"/g, "").trim();

const container = dom.window.document.createElement("div");
for (const [name, { codicon, size }] of Object.entries(expected)) {
  const value = icons[name];
  check(`${name} is a non-empty string`, typeof value === "string" && value.length > 0);
  if (typeof value !== "string") continue;

  const source = await readFile(path.join(codiconDir, "src", "icons", `${codicon}.svg`), "utf8");
  check(
    `${name} equals ${codicon}.svg apart from the size attributes`,
    withoutSize(value) === withoutSize(source),
  );

  container.innerHTML = value;
  const svg = container.firstElementChild;
  check(`${name} parses to a single svg element`, container.children.length === 1 && svg?.tagName.toLowerCase() === "svg");
  if (!svg || svg.tagName.toLowerCase() !== "svg") continue;

  check(`${name} keeps its width of ${size}`, svg.getAttribute("width") === String(size));
  check(`${name} keeps its height of ${size}`, svg.getAttribute("height") === String(size));
  check(`${name} keeps the 16-unit codicon viewBox`, svg.getAttribute("viewBox") === "0 0 16 16");
  check(`${name} is a filled glyph in currentColor`, svg.getAttribute("fill") === "currentColor");
  check(`${name} carries no stroke attributes`, svg.getAttribute("stroke") === null);
  check(`${name} contains at least one drawing element`, svg.children.length > 0);
}

if (failures.length > 0) {
  console.error(`icons: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("icons: all assertions passed");
