// The model picker trigger (src/ui/model-picker-trigger.ts) in jsdom: a
// pill button showing the selected model's id. Clicking opens a
// DropdownMenu of the ModelService catalog; picking one sends the select
// command through the service and leaves the label for the server's
// confirming snapshot. The trigger re-renders on the service's change
// events; dispose() closes an open menu and unsubscribes. The service
// under test is a real ModelService with a recording send function - its
// constructor takes nothing else. Runs under the shared leak check: an
// undisposed trigger or service fails.
// Run: node test/model-picker-trigger.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { ModelService } from "./src/services/model-service.ts";
      export { ModelPickerTrigger } from "./src/ui/model-picker-trigger.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

// lucide's createElement renders the chevron against the DOM, so the
// jsdom globals must exist before the bundle is imported.
const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.HTMLButtonElement = dom.window.HTMLButtonElement;
globalThis.Element = dom.window.Element;
globalThis.Node = dom.window.Node;
globalThis.CustomEvent = dom.window.CustomEvent;

const bundlePath = path.join(os.tmpdir(), "promptforge-model-picker-trigger-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { ModelPickerTrigger, ModelService, lifecycle } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function menuEl() {
  return document.querySelector(".menu-popup");
}

function menuItems() {
  return [...(menuEl()?.querySelectorAll(".menu-item") ?? [])];
}

function labelOf(trigger) {
  return trigger.element.querySelector(".model-picker-trigger__label")?.textContent;
}

await assertNoLeaks(lifecycle, async () => {
  // --- The trigger shows the current model ---------------------------------

  {
    const service = new ModelService(() => true);
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    check(
      "the trigger is a type=button pill",
      trigger.element.tagName === "BUTTON" && trigger.element.type === "button",
    );
    check(
      "the trigger carries the model-picker-trigger class",
      trigger.element.classList.contains("model-picker-trigger"),
    );
    check("no selection shows the placeholder label", labelOf(trigger) === "Select model");
    check("no selection carries no tooltip", trigger.element.getAttribute("title") === null);
    service.applySelected("alpha");
    check("the trigger shows the current model id", labelOf(trigger) === "alpha");
    trigger.dispose();
    service.dispose();
    trigger.element.remove();
  }

  {
    const service = new ModelService(() => true);
    service.applySelected("beta");
    const trigger = new ModelPickerTrigger(service);
    check("the initial render reads the service's selection", labelOf(trigger) === "beta");
    trigger.dispose();
    service.dispose();
  }

  // --- The dropdown lists the catalog ----------------------------------------

  {
    const service = new ModelService(() => true);
    service.setModels([{ id: "alpha", description: "the alpha model" }, { id: "beta" }]);
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    trigger.element.click();
    check("clicking the trigger opens the dropdown", menuEl() !== null);
    const items = menuItems();
    check(
      "the dropdown lists the catalog in order",
      items.length === 2 &&
        items[0]?.textContent === "alpha" &&
        items[1]?.textContent === "beta",
    );
    check(
      "the trigger gains the menu's aria wiring",
      trigger.element.getAttribute("aria-haspopup") === "menu" &&
        trigger.element.getAttribute("aria-expanded") === "true",
    );
    trigger.dispose();
    service.dispose();
    trigger.element.remove();
  }

  {
    const sent = [];
    const service = new ModelService((id) => (sent.push(id), true));
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    trigger.element.click();
    const items = menuItems();
    check(
      "an empty catalog lists a single no-models row",
      items.length === 1 && items[0]?.textContent === "No models available",
    );
    items[0]?.click();
    check(
      "the no-models row is inert",
      sent.length === 0 && menuEl() === null,
    );
    trigger.dispose();
    service.dispose();
    trigger.element.remove();
  }

  // --- Selection sends the command through the service ------------------------

  {
    const sent = [];
    const service = new ModelService((id) => (sent.push(id), true));
    service.setModels([{ id: "alpha" }, { id: "beta" }]);
    service.applySelected("alpha");
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    trigger.element.click();
    menuItems()[1]?.click();
    check(
      "selecting a model sends the select command with its id",
      sent.join(",") === "beta",
    );
    check("selecting a model closes the dropdown", menuEl() === null);
    check(
      "the label waits for the snapshot instead of updating optimistically",
      labelOf(trigger) === "alpha",
    );
    service.applySelected("beta");
    check("the confirming snapshot updates the label", labelOf(trigger) === "beta");
    trigger.dispose();
    service.dispose();
    trigger.element.remove();
  }

  // --- Reactive updates ---------------------------------------------------------

  {
    const service = new ModelService(() => true);
    service.setModels([{ id: "alpha", description: "the alpha model" }]);
    service.applySelected("alpha");
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    check(
      "the tooltip resolves the current model's description from the catalog",
      trigger.element.title === "the alpha model",
    );
    service.setModels([{ id: "alpha", description: "the renamed alpha" }, { id: "beta" }]);
    check("a catalog change re-resolves the tooltip", trigger.element.title === "the renamed alpha");
    service.applySelected("beta");
    check(
      "a selection without a description clears the tooltip",
      trigger.element.getAttribute("title") === null,
    );
    service.applySelected("gamma");
    check(
      "a selection absent from the catalog shows its id with no tooltip",
      labelOf(trigger) === "gamma" && trigger.element.getAttribute("title") === null,
    );
    trigger.dispose();
    service.dispose();
    trigger.element.remove();
  }

  // --- Dispose ---------------------------------------------------------------------

  {
    const service = new ModelService(() => true);
    service.setModels([{ id: "alpha" }]);
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    trigger.element.click();
    check("a menu is open before dispose", menuEl() !== null);
    trigger.dispose();
    check("dispose closes the open menu", menuEl() === null);
    check(
      "dispose restores the trigger's aria state",
      trigger.element.getAttribute("aria-expanded") === null,
    );
    service.applySelected("alpha");
    check("a disposed trigger stops reacting to the service", labelOf(trigger) === "Select model");
    trigger.element.click();
    check("a disposed trigger does not reopen its menu", menuEl() === null);
    service.dispose();
    trigger.element.remove();
  }
});

if (failures.length > 0) {
  console.error(`model-picker-trigger: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("model-picker-trigger: all assertions passed");
process.exit(0);
