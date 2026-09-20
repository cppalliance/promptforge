// Unit test for the shared status bar shell (shared-ui/status-bar.ts):
// the slot swap between the inline progress bar and the consumer's
// indicators group (progress wins, null restores, the group's contents
// survive the swap), the zero-total clamp, the text region's label,
// tooltip, and error styling, and the extras region the consumers fill.
// Bundles the module with esbuild and drives it against jsdom.
// Run: node test/shared-status-bar.mjs.
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "node_modules", "shared-ui", "status-bar.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module imports its colocated CSS; the test drives only the JS,
  // and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});
const { createStatusBarShell } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const shell = createStatusBarShell();
window.document.body.append(shell.element);

// A consumer's indicator: the swap must never touch its contents.
const led = window.document.createElement("span");
led.className = "status-bar__led";
shell.indicators.append(led);

// --- The shell's structure ----------------------------------------------------

check("the element is the status-bar footer", shell.element.matches("footer.status-bar"));
check("the bar is a polite live region", shell.element.getAttribute("aria-live") === "polite");
check("the progress bar starts hidden", shell.progress.hidden === true);
check("the indicators group starts visible", shell.indicators.hidden === false);
check("the text region starts empty", shell.text.textContent === "");
check("the extras region is empty until the consumer fills it", shell.extras.childElementCount === 0);

// --- The slot swap --------------------------------------------------------------

shell.renderSlot({ current: 1, total: 4 });
check("a reading reveals the progress bar", shell.progress.hidden === false);
check("a reading hides the indicators group", shell.indicators.hidden === true);
check(
  "the bar shows the reading",
  shell.progress.value === 1 && shell.progress.max === 4,
);
check("the swap kept the consumer's LED in the group", shell.indicators.contains(led));

shell.renderSlot({ current: 2, total: 4 });
check("a second reading updates the bar in place", shell.progress.value === 2);

shell.renderSlot({ current: 0, total: 0 });
check("a zero total clamps max so value/max stay valid", shell.progress.max === 1);

shell.renderSlot(null);
check("clearing progress hides the bar", shell.progress.hidden === true);
check("clearing progress restores the indicators group", shell.indicators.hidden === false);
check("the restored group still carries the consumer's LED", shell.indicators.contains(led));

// --- The text region --------------------------------------------------------------

shell.setText("Downloading model", { tooltip: "1 of 2" });
check("setText sets the label", shell.text.textContent === "Downloading model");
check("setText rides the tooltip on the bar", shell.element.title === "1 of 2");
check("the error styling starts off", !shell.text.classList.contains("status-bar__text--error"));

shell.setText("The download failed", { error: true });
check("an error label takes the error styling", shell.text.classList.contains("status-bar__text--error"));
check("a missing tooltip clears the bar's title", shell.element.title === "");

shell.setText("Ready", { error: false });
check("a later setText clears the error styling", !shell.text.classList.contains("status-bar__text--error"));

if (failures.length > 0) {
  console.error(`shared-status-bar: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-status-bar: all assertions passed");
