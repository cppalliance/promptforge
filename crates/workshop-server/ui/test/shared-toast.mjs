// Unit test for the shared toast stack (shared-ui/toast.ts), which the
// workshop's update notifications now ride: show() appends a kind-classed
// toast to the polite live region, and the toast dismisses itself after
// its four-second lifetime. Bundles the module with esbuild and drives it
// against jsdom with mocked timers.
// Run: node test/shared-toast.mjs.
import { mock } from "node:test";
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
  entryPoints: [path.join(uiDir, "..", "node_modules", "shared-ui", "toast.ts")],
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
const { createToastStack } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

mock.timers.enable({ apis: ["setTimeout"] });

const toasts = createToastStack();
window.document.body.append(toasts.element);

check("the stack is a polite live region", toasts.element.getAttribute("aria-live") === "polite");
check("the stack announces as status", toasts.element.getAttribute("role") === "status");

toasts.show("PromptForge 1.2.3 is available", "info");
toasts.show("Update failed: boom", "error");
const shown = [...toasts.element.querySelectorAll(".toast")];
check("show appends one toast per call", shown.length === 2);
check("the toast carries its kind class", shown[0]?.classList.contains("toast-info") && shown[1]?.classList.contains("toast-error"));
check("the toast renders its message", shown[0]?.textContent === "PromptForge 1.2.3 is available");

mock.timers.tick(3999);
check("a toast stays for its full lifetime", toasts.element.querySelectorAll(".toast").length === 2);
mock.timers.tick(1);
check("a toast dismisses itself after its lifetime", toasts.element.querySelectorAll(".toast").length === 0);

mock.timers.reset();

if (failures.length > 0) {
  console.error(`shared-toast: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shared-toast: all assertions passed");
