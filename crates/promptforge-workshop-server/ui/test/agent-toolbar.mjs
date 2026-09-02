// The agent toolbar (src/ui/agent-toolbar.ts) in jsdom: a role=toolbar
// flex row composing ModeChip, ModelPickerTrigger, and TokenRing - chip
// and picker leading, the ring last so the stylesheet can pin it to the
// trailing edge. The picker reads the constructor's ModelService;
// dispose() cascades to all three children. jsdom has no layout engine,
// so spacing and alignment pin against the stylesheet source (the
// titlebar-style.mjs pattern). Runs under the shared leak check: an
// undisposed toolbar or child fails.
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
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { ModelService } from "./src/services/model-service.ts";
      export { AgentToolbar } from "./src/ui/agent-toolbar.ts";
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

// lucide's createElement renders the chip and picker icons against the
// DOM, so the jsdom globals must exist before the bundle is imported.
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
  return document.querySelector(".workshop-dropdown");
}

await assertNoLeaks(lifecycle, async () => {
  // --- The container -----------------------------------------------------------

  {
    const service = new ModelService(() => true);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    check(
      "the toolbar carries the agent-toolbar class",
      toolbar.element.classList.contains("agent-toolbar"),
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
        children[0]?.classList.contains("mode-chip") &&
        children[1]?.classList.contains("model-picker-trigger") &&
        children[2]?.classList.contains("token-ring"),
    );
    check(
      "the ring is the last child so the stylesheet can pin it to the trailing edge",
      toolbar.element.lastElementChild?.classList.contains("token-ring") === true,
    );
    check(
      "each child renders its own control",
      toolbar.element.querySelector(".mode-chip__label")?.textContent === "Agent" &&
        toolbar.element.querySelector(".model-picker-trigger__label")?.textContent ===
          "Select model" &&
        toolbar.element.querySelector(".token-ring")?.getAttribute("aria-valuenow") ===
          "0",
    );
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
      toolbar.element.querySelector(".model-picker-trigger__label")?.textContent ===
        "alpha",
    );
    toolbar.dispose();
    service.dispose();
    toolbar.element.remove();
  }

  // --- Dispose cascades ------------------------------------------------------------

  {
    const service = new ModelService(() => true);
    const toolbar = new AgentToolbar(service);
    document.body.appendChild(toolbar.element);
    const chip = toolbar.element.querySelector(".mode-chip");
    const picker = toolbar.element.querySelector(".model-picker-trigger");
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

  // --- The stylesheet contract -------------------------------------------------------
  // jsdom has no layout engine, so spacing and alignment pin against the
  // CSS source.

  const cssText = (
    await readFile(path.join(testDir, "..", "src", "ui", "agent-toolbar.css"), "utf8")
  ).replace(/\/\*[\s\S]*?\*\//g, "");

  // Returns the declaration block of the first rule whose selector list
  // contains `selector` exactly, or "" when no rule carries it.
  function ruleBlock(selector) {
    let from = 0;
    for (;;) {
      const start = cssText.indexOf(selector, from);
      if (start === -1) return "";
      let i = start + selector.length;
      while (i < cssText.length && /\s/.test(cssText[i])) i += 1;
      if (cssText[i] === "{" || cssText[i] === ",") {
        const open = cssText.indexOf("{", i);
        const end = open === -1 ? -1 : cssText.indexOf("}", open);
        return end === -1 ? "" : cssText.slice(open + 1, end);
      }
      from = start + selector.length;
    }
  }

  const toolbarRule = ruleBlock(".agent-toolbar");
  check(
    "the toolbar lays out as a centered flex row",
    toolbarRule.includes("display: flex") && toolbarRule.includes("align-items: center"),
  );
  check(
    "the toolbar uses Cursor's workspace input-row gap",
    toolbarRule.includes("gap: 0.55rem"),
  );
  check(
    "the toolbar stands at base control height",
    toolbarRule.includes("min-block-size: var(--height-base)"),
  );
  check(
    "the ring pins to the trailing edge with a logical property",
    ruleBlock(".agent-toolbar > .token-ring").includes("margin-inline-start: auto"),
  );
  const varUses = cssText.match(/var\([^)]*\)/g) ?? [];
  check(
    "no var() in the toolbar stylesheet carries a fallback",
    varUses.length > 0 && varUses.every((use) => !use.includes(",")),
  );
});

if (failures.length > 0) {
  console.error(`agent-toolbar: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("agent-toolbar: all assertions passed");
process.exit(0);
