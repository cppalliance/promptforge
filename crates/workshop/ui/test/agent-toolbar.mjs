// The agent toolbar (src/parts/agent/agent-toolbar.ts) in jsdom: a role=toolbar
// flex row composing ModeChip, ModelPickerTrigger, and TokenRing. The picker
// reads the constructor's ModelService; the toolbar forwards the chip's mode
// and change event and the two menu openers the composer's keys use;
// dispose() cascades to all three children. The stylesheet's side - a fixed
// 28px row with 4px gaps, the model button pushing the ring and the action
// button to the trailing edge - is read from the source text. Runs under the
// shared leak check: an undisposed toolbar or child fails.
// Run: node test/agent-toolbar.mjs
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
      export { AgentToolbar } from "./src/parts/agent/agent-toolbar.ts";
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

// The children read the DOM globals when they build, so the jsdom globals
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

const bundlePath = path.join(os.tmpdir(), "promptforge-agent-toolbar-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { AgentToolbar, ModelService, lifecycle } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function menuEl() {
  return document.querySelector(".menu-popup");
}

await assertNoLeaks(lifecycle, async () => {
  // --- The container -----------------------------------------------------------

  {
    const service = new ModelService(() => true);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    check(
      "the toolbar has the ws-agent-toolbar class",
      toolbar.element.classList.contains("ws-agent-toolbar"),
    );
    check(
      "the toolbar is a named toolbar landmark",
      toolbar.element.getAttribute("role") === "toolbar" &&
        toolbar.element.getAttribute("aria-label") === "Agent controls",
    );
    toolbar.dispose();
    service.dispose();
    toolbar.element.remove();
  }

  // --- The children --------------------------------------------------------------

  {
    const service = new ModelService(() => true);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    const children = [...toolbar.element.children];
    check(
      "the toolbar composes the chip, the picker, and the ring in order",
      children.length === 3 &&
        children[0]?.classList.contains("ws-mode-chip") &&
        children[1]?.classList.contains("ws-model-picker-trigger") &&
        children[2]?.classList.contains("ws-token-ring"),
    );
    check(
      "the ring is the last child so the stylesheet can pin it to the trailing edge",
      toolbar.element.lastElementChild?.classList.contains("ws-token-ring") === true,
    );
    check(
      "each child renders its own control",
      toolbar.element.querySelector(".ws-mode-chip__label")?.textContent === "Agent" &&
        toolbar.element.querySelector(".ws-model-picker-trigger__label")?.textContent ===
          "Select model" &&
        toolbar.element.querySelector(".ws-token-ring")?.hasAttribute("hidden") === true,
    );
    for (const button of toolbar.element.querySelectorAll("button")) {
      button.focus();
      check(
        `${button.getAttribute("aria-label") ?? button.textContent.trim()} accepts keyboard focus`,
        document.activeElement === button && button.matches(":focus-visible"),
      );
    }
    toolbar.dispose();
    service.dispose();
    toolbar.element.remove();
  }

  // --- The service threads into the picker -----------------------------------------

  {
    const service = new ModelService(() => true);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    service.applySelected("alpha");
    check(
      "the picker reads the toolbar's model service",
      toolbar.element.querySelector(".ws-model-picker-trigger__label")?.textContent ===
        "alpha",
    );
    toolbar.dispose();
    service.dispose();
    toolbar.element.remove();
  }

  // --- The mode and the menu openers ---------------------------------------------------

  {
    const service = new ModelService(() => true);
    service.setModels([{ id: "alpha" }]);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    const seen = [];
    toolbar.onDidChangeMode((mode) => seen.push(mode));
    check("the toolbar reports the chip's mode", toolbar.mode === "agent");
    toolbar.openModeMenu();
    check("openModeMenu opens the mode menu", menuEl()?.classList.contains("ws-mode-menu") === true);
    toolbar.openModeMenu();
    check(
      "a second openModeMenu cycles the mode and the change event follows",
      toolbar.mode === "plan" && seen.join(",") === "plan",
    );
    toolbar.element.querySelector(".ws-mode-chip")?.click();
    check("the menu closes before the model menu opens", menuEl() === null);
    toolbar.openModelMenu();
    check("openModelMenu opens the model menu", menuEl()?.classList.contains("ws-model-menu") === true);
    toolbar.dispose();
    service.dispose();
    toolbar.element.remove();
  }

  // --- Dispose cascades ------------------------------------------------------------

  {
    const service = new ModelService(() => true);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    const chip = toolbar.element.querySelector(".ws-mode-chip");
    const picker = toolbar.element.querySelector(".ws-model-picker-trigger");
    chip?.click();
    check("a chip menu is open before dispose", menuEl() !== null);
    toolbar.dispose();
    check("dispose closes the chip's open menu", menuEl() === null);
    picker?.click();
    check("a disposed toolbar's picker does not reopen its menu", menuEl() === null);
    let serviceFired = false;
    service.onDidChangeCurrent(() => {
      serviceFired = true;
    });
    service.applySelected("beta");
    check(
      "a disposed toolbar leaves the borrowed model service alive",
      serviceFired === true,
    );
    service.dispose();
    toolbar.element.remove();
  }
});

// --- The stylesheet --------------------------------------------------------------------

{
  const css = (
    await readFile(path.join(testDir, "..", "src", "parts", "agent", "agent-toolbar.css"), "utf8")
  ).replace(/\/\*[\s\S]*?\*\//g, "");
  const componentCss = await readFile(path.join(testDir, "..", "src", "tokens", "component.css"), "utf8");
  check("the toolbar is a fixed 28px row", /\.ws-agent-toolbar\s*\{[^}]*block-size:\s*var\(--height-base\)/.test(css));
  check(
    "the toolbar's children sit 4px apart",
    /gap:\s*var\(--ws-agent-toolbar-gap\)/.test(css) && /--ws-agent-toolbar-gap:\s*var\(--ws-size-4\)/.test(componentCss),
  );
  check(
    "the model button pushes the ring and the action button to the trailing edge",
    /\.ws-agent-toolbar > \.ws-model-picker-trigger\s*\{[^}]*margin-inline-end:\s*auto/.test(css),
  );
}

if (failures.length > 0) {
  console.error(`ws-agent-toolbar: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ws-agent-toolbar: all assertions passed");
process.exit(0);
