// Unit test for the shared status bar shell (shared-ui/status-bar.ts):
// the barberpole beside the consumer's indicators group (setBusy shows
// and hides the barberpole, the group stays visible throughout and keeps
// its contents, the barberpole precedes the group in DOM order), the
// text region's label, tooltip, and error styling, and the extras region
// the consumers fill. Bundles the module with esbuild and drives it
// against jsdom.
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

// A consumer's indicator: busy toggling must never touch its contents.
const led = window.document.createElement("span");
led.className = "status-bar__led";
shell.indicators.append(led);

// --- The shell's structure ----------------------------------------------------

check("the element is the status-bar footer", shell.element.matches("footer.status-bar"));
check("the bar is a polite live region", shell.element.getAttribute("aria-live") === "polite");
check(
  "the barberpole is the shell's element of that class",
  shell.barberpole === shell.element.querySelector(".status-bar__barberpole"),
);
check("the barberpole starts hidden", shell.barberpole.hidden === true);
check("the indicators group starts visible", shell.indicators.hidden === false);
check(
  "the barberpole sits in the right group",
  shell.barberpole.parentElement?.matches(".status-bar__right") === true,
);
check(
  "the barberpole precedes the indicators group in DOM order",
  (shell.barberpole.compareDocumentPosition(shell.indicators) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
);
check(
  "the barberpole is an indeterminate progressbar to assistive tech",
  shell.barberpole.getAttribute("role") === "progressbar" &&
    !shell.barberpole.hasAttribute("aria-valuenow"),
);
check("no <progress> element remains in the shell", shell.element.querySelector("progress") === null);
check("the text region starts empty", shell.text.textContent === "");
check("the extras region is empty until the consumer fills it", shell.extras.childElementCount === 0);

// --- The busy toggle --------------------------------------------------------------

shell.setBusy(true);
check("setBusy(true) shows the barberpole", shell.barberpole.hidden === false);
check("setBusy(true) leaves the indicators group visible", shell.indicators.hidden === false);
check("setBusy(true) kept the consumer's LED in the group", shell.indicators.contains(led));

shell.setBusy(true);
check("a repeated setBusy(true) keeps the barberpole shown", shell.barberpole.hidden === false);

shell.setBusy(false);
check("setBusy(false) hides the barberpole", shell.barberpole.hidden === true);
check("setBusy(false) leaves the indicators group visible", shell.indicators.hidden === false);
check("the group still holds the consumer's LED", shell.indicators.contains(led));
check("the shell exposes no renderSlot", typeof shell.renderSlot === "undefined");
check("the shell exposes no progress element", typeof shell.progress === "undefined");

// --- The text region --------------------------------------------------------------

shell.setText("Downloading model", { tooltip: "1 of 2" });
check("setText sets the label", shell.text.textContent === "Downloading model");
check("setText sets the tooltip on the bar", shell.element.title === "1 of 2");
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
