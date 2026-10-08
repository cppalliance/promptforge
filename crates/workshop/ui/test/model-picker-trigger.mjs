// The model picker trigger (src/parts/chrome/model-picker-trigger.ts) in jsdom: a
// button showing the selected model's id and a 9px chevron, with the tooltip
// "Switch Model (Ctrl+/)". Clicking (or open(), which Ctrl+/ calls) opens a
// DropdownMenu of the ModelService catalog on the composer's menu surface with
// a check-only selection, 230px wide and at most 320px tall, reading "No models
// found" when the catalog is empty; picking one sends the select command
// through the service and leaves the label for the server's confirming
// snapshot. The trigger re-renders when the service's selection changes (a
// catalog change leaves the label alone); dispose()
// closes an open menu and unsubscribes. The stylesheet's side - 20px tall, a
// 4px radius, 6px side padding, 13px/18px secondary text, an 8% hover fill, the
// chevron at 0.7 - is read from the source text. The service under test is a
// real ModelService with a recording send function - its constructor takes
// nothing else. Runs under the shared leak check: an undisposed trigger or
// service fails.
// Run: node test/model-picker-trigger.mjs
import { readFile, writeFile } from "node:fs/promises";
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
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { ModelService } from "./src/services/model-service.ts";
      export { ModelPickerTrigger } from "./src/parts/chrome/model-picker-trigger.ts";
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

// The trigger reads the DOM globals when it builds, so the jsdom globals
// must exist before the bundle is imported.
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
  return trigger.element.querySelector(".ws-model-picker-trigger__label")?.textContent;
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
      "the trigger has the ws-model-picker-trigger class",
      trigger.element.classList.contains("ws-model-picker-trigger"),
    );
    check("no selection shows the placeholder label", labelOf(trigger) === "Select model");
    check(
      "the tooltip is Switch Model (Ctrl+/) with or without a selection",
      trigger.element.title === "Switch Model (Ctrl+/)",
    );
    check(
      "the chevron is a codicon",
      trigger.element.querySelector(".ws-model-picker-trigger__icon svg") !== null,
    );
    service.applySelected("alpha");
    check("the trigger shows the current model id", labelOf(trigger) === "alpha");
    check("the tooltip stays the same after a selection", trigger.element.title === "Switch Model (Ctrl+/)");
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
    service.applySelected("beta");
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    trigger.element.click();
    check("clicking the trigger opens the dropdown", menuEl() !== null);
    const items = menuItems();
    check(
      "the dropdown lists the catalog in order",
      items.length === 2 &&
        items[0]?.querySelector(".menu-item__label")?.textContent === "alpha" &&
        items[1]?.querySelector(".menu-item__label")?.textContent === "beta",
    );
    check(
      "the trigger gains the menu's aria wiring",
      trigger.element.getAttribute("aria-haspopup") === "menu" &&
        trigger.element.getAttribute("aria-expanded") === "true",
    );
    check(
      "the menu wears the composer surface at the model menu's size",
      menuEl()?.classList.contains("menu-composer") === true &&
        menuEl()?.classList.contains("ws-model-menu") === true,
    );
    check(
      "the selection is a check on the current model alone",
      items[1]?.querySelector(".menu-item__check svg") !== null &&
        items[0]?.querySelector(".menu-item__check") === null,
    );
    trigger.dispose();
    service.dispose();
    trigger.element.remove();
  }

  {
    // Ctrl+/ opens the same menu through open(); a second call leaves it open.
    const service = new ModelService(() => true);
    service.setModels([{ id: "alpha" }]);
    const trigger = new ModelPickerTrigger(service);
    document.body.appendChild(trigger.element);
    trigger.open();
    check("open() opens the model menu", menuEl() !== null);
    trigger.open();
    check("open() while open keeps the menu open", menuEl() !== null);
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
      items.length === 1 && items[0]?.textContent === "No models found",
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
    check("the label follows the selection", labelOf(trigger) === "alpha");
    service.setModels([{ id: "alpha", description: "the renamed alpha" }, { id: "beta" }]);
    check("a catalog change leaves the label and the fixed tooltip alone", labelOf(trigger) === "alpha" && trigger.element.title === "Switch Model (Ctrl+/)");
    service.applySelected("beta");
    check("a new selection updates the label", labelOf(trigger) === "beta");
    service.applySelected("gamma");
    check(
      "a selection absent from the catalog shows its id",
      labelOf(trigger) === "gamma" && trigger.element.title === "Switch Model (Ctrl+/)",
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

// --- The stylesheet --------------------------------------------------------------------

{
  const css = (
    await readFile(path.join(testDir, "..", "src", "parts", "chrome", "model-picker-trigger.css"), "utf8")
  ).replace(/\/\*[\s\S]*?\*\//g, "");
  const lookTokens = await readFile(path.join(testDir, "..", "..", "look", "tokens.css"), "utf8");
  const rule = (selector) => {
    const match = new RegExp(`${selector.replace(/[.[\]"=]/g, "\\$&")}\\s*\\{([^}]*)\\}`).exec(css);
    return match === null ? "" : match[1].replace(/\s+/g, " ");
  };
  const trigger = rule(".ws-model-picker-trigger");
  check("the button is 20px tall", /block-size:\s*var\(--model-trigger-height\)/.test(trigger) && /--model-trigger-height:\s*20px/.test(lookTokens));
  check("the button has a 4px radius", /border-radius:\s*var\(--model-trigger-radius\)/.test(trigger) && /--model-trigger-radius:\s*4px/.test(lookTokens));
  check("the button pads 6px at its sides", /padding-inline:\s*var\(--space-1-5\)/.test(trigger));
  check(
    "the text is 13px/18px in the secondary tier",
    /font-size:\s*var\(--font-size-base\)/.test(trigger) &&
      /line-height:\s*var\(--line-height-base\)/.test(trigger) &&
      /color:\s*var\(--cursor-text-secondary\)/.test(trigger),
  );
  check(
    "hover fills 8%",
    /\.ws-model-picker-trigger:hover[^{]*\{[^}]*var\(--cursor-bg-tertiary\)/.test(css),
  );
  check(
    "the chevron is 9px at 0.7 opacity",
    /opacity:\s*0\.7/.test(rule(".ws-model-picker-trigger__icon")) &&
      /inline-size:\s*var\(--ws-model-chevron-size\)/.test(rule(".ws-model-picker-trigger__icon svg")),
  );
  check(
    "the menu is 230px wide and at most 320px tall",
    /inline-size:\s*var\(--ws-model-menu-width\)/.test(rule(".ws-model-menu")) &&
      /max-block-size:\s*var\(--ws-model-menu-max-height\)/.test(rule(".ws-model-menu")),
  );
}

if (failures.length > 0) {
  console.error(`ws-model-picker-trigger: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ws-model-picker-trigger: all assertions passed");
process.exit(0);
