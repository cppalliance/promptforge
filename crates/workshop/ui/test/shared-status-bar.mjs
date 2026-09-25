// Unit test for the shared status bar view (shared-ui/status-bar.ts):
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
const { createStatusBarView } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const view = createStatusBarView();
window.document.body.append(view.element);

// A consumer's indicator: busy toggling must never touch its contents.
const led = window.document.createElement("span");
led.className = "status-bar__led";
view.indicators.append(led);

// --- The view's structure ----------------------------------------------------

check("the element is the status-bar footer", view.element.matches("footer.status-bar"));
check("the bar is a polite live region", view.element.getAttribute("aria-live") === "polite");
check(
  "the barberpole is the view's element of that class",
  view.barberpole === view.element.querySelector(".status-bar__barberpole"),
);
check("the barberpole starts hidden", view.barberpole.hidden === true);
check("the indicators group starts visible", view.indicators.hidden === false);
check(
  "the barberpole sits in the right group",
  view.barberpole.parentElement?.matches(".status-bar__right") === true,
);
check(
  "the barberpole precedes the indicators group in DOM order",
  (view.barberpole.compareDocumentPosition(view.indicators) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
);
check(
  "the barberpole is an indeterminate progressbar to assistive tech",
  view.barberpole.getAttribute("role") === "progressbar" &&
    !view.barberpole.hasAttribute("aria-valuenow"),
);
check("no <progress> element remains in the view", view.element.querySelector("progress") === null);
check("the text region starts empty", view.text.textContent === "");
check("the extras region is empty until the consumer fills it", view.extras.childElementCount === 0);

// --- The busy toggle --------------------------------------------------------------

view.setBusy(true);
check("setBusy(true) shows the barberpole", view.barberpole.hidden === false);
check("setBusy(true) leaves the indicators group visible", view.indicators.hidden === false);
check("setBusy(true) kept the consumer's LED in the group", view.indicators.contains(led));

view.setBusy(true);
check("a repeated setBusy(true) keeps the barberpole shown", view.barberpole.hidden === false);

view.setBusy(false);
check("setBusy(false) hides the barberpole", view.barberpole.hidden === true);
check("setBusy(false) leaves the indicators group visible", view.indicators.hidden === false);
check("the group still holds the consumer's LED", view.indicators.contains(led));
check("the view exposes no renderSlot", typeof view.renderSlot === "undefined");
check("the view exposes no progress element", typeof view.progress === "undefined");

// --- The text region --------------------------------------------------------------

view.setText("Downloading model", { tooltip: "1 of 2" });
check("setText sets the label", view.text.textContent === "Downloading model");
check("setText sets the tooltip on the bar", view.element.title === "1 of 2");
check("the error styling starts off", !view.text.classList.contains("status-bar__text--error"));

view.setText("The download failed", { error: true });
check("an error label takes the error styling", view.text.classList.contains("status-bar__text--error"));
check("a missing tooltip clears the bar's title", view.element.title === "");

view.setText("Ready", { error: false });
check("a later setText clears the error styling", !view.text.classList.contains("status-bar__text--error"));

if (failures.length > 0) {
  console.error(`shared-status-bar: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-status-bar: all assertions passed");
