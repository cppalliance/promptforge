// Unit test for the shared progress bar (progress.ts): the settings variant
// is the default and the workbench variant is a modifier class; a null
// fraction renders the indeterminate bar (a class the stylesheet animates)
// with no aria value, and any number clears it again; fractions clamp to
// 0..1 and set the fill's --progress scale and the aria value. Bundles the
// module with esbuild and drives it against jsdom.
// Run: node test/shared-progress.mjs (from crates/workshop/look).
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
globalThis.window = dom.window;
globalThis.document = dom.window.document;

const bundle = await esbuild.build({
  entryPoints: [path.join(lookDir, "progress.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module imports its colocated CSS; the test drives only the JS.
  loader: { ".css": "empty" },
});
const { createProgressBar } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const fillOf = (bar) => bar.element.querySelector(".progress__fill");

// --- Structure and the default variant --------------------------------------------

const bar = createProgressBar("Update download progress");
const el = bar.element;
check("the track is a progressbar", el.getAttribute("role") === "progressbar");
check("the track is labeled", el.getAttribute("aria-label") === "Update download progress");
check("the track has the progress class", el.classList.contains("progress"));
check("the default variant adds no modifier", [...el.classList].join(" ") === "progress");
check("the track holds one fill", el.children.length === 1 && fillOf(bar) !== null);

// --- Determinate ------------------------------------------------------------------

bar.setFraction(0.25);
check("a fraction scales the fill", fillOf(bar).style.getPropertyValue("--progress") === "0.25");
check("a fraction sets the aria value", el.getAttribute("aria-valuenow") === "25");
check("a fraction is not indeterminate", !el.classList.contains("progress--indeterminate"));
bar.setFraction(1.7);
check("a fraction above 1 clamps to full", fillOf(bar).style.getPropertyValue("--progress") === "1" && el.getAttribute("aria-valuenow") === "100");
bar.setFraction(-3);
check("a fraction below 0 clamps to empty", fillOf(bar).style.getPropertyValue("--progress") === "0" && el.getAttribute("aria-valuenow") === "0");

// --- Indeterminate ----------------------------------------------------------------

bar.setFraction(null);
check("a null fraction is indeterminate", el.classList.contains("progress--indeterminate"));
check("a null fraction drops the aria value", !el.hasAttribute("aria-valuenow"));
bar.setFraction(0.5);
check("a number after null clears the indeterminate state", !el.classList.contains("progress--indeterminate"));
check("a number after null restores the aria value", el.getAttribute("aria-valuenow") === "50");

// --- The workbench variant --------------------------------------------------------

const workbench = createProgressBar("Loading", "workbench");
check("the workbench variant takes the modifier class", workbench.element.classList.contains("progress--workbench"));
check("the workbench variant keeps the progress class", workbench.element.classList.contains("progress"));
workbench.setFraction(null);
check("the workbench variant goes indeterminate on null", workbench.element.classList.contains("progress--indeterminate"));

if (failures.length > 0) {
  console.error(`shared-progress: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-progress: all assertions passed");
