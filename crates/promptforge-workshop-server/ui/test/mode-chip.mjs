// The mode chip (src/ui/mode-chip.ts) in jsdom: a button showing the
// current mode's icon and label. Clicking opens a DropdownMenu of the
// four modes; picking one updates the chip and fires
// "agent-mode-changed" on document with the mode as detail; re-picking
// the current mode fires nothing; dispose() closes an open menu. Runs
// under the shared leak check: a ModeChip left undisposed fails.
// Run: node test/mode-chip.mjs
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
      export { AGENT_MODE_CHANGED_EVENT, ModeChip, UNIFIED_MODES } from "./src/ui/mode-chip.ts";
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

// lucide's createElement renders the mode icons at module load, so the
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

const bundlePath = path.join(os.tmpdir(), "promptforge-mode-chip-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { AGENT_MODE_CHANGED_EVENT, ModeChip, UNIFIED_MODES, lifecycle } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function menuEl() {
  return document.querySelector(".workshop-dropdown");
}

function menuItems() {
  return [...(menuEl()?.querySelectorAll(".workshop-dropdown__item") ?? [])];
}

await assertNoLeaks(lifecycle, async () => {
  // --- The mode constants ----------------------------------------------------

  check(
    "the modes are the as-const map the plan fixes",
    UNIFIED_MODES.Agent === "agent" &&
      UNIFIED_MODES.Ask === "ask" &&
      UNIFIED_MODES.Plan === "plan" &&
      UNIFIED_MODES.Debug === "debug",
  );
  check(
    "the mode-changed event name is the literal the plan fixes",
    AGENT_MODE_CHANGED_EVENT === "agent-mode-changed",
  );

  // --- The trigger -----------------------------------------------------------

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    check(
      "the chip is a type=button trigger",
      chip.element.tagName === "BUTTON" && chip.element.type === "button",
    );
    check(
      "the chip carries the mode-chip class",
      chip.element.classList.contains("mode-chip"),
    );
    check("the chip starts on the agent mode", chip.mode === "agent");
    check(
      "the chip shows the current mode's label and icon",
      chip.element.querySelector(".mode-chip__label")?.textContent === "Agent" &&
        chip.element.querySelector(".mode-chip__icon svg") !== null,
    );
    chip.dispose();
    chip.element.remove();
  }

  // --- The dropdown ------------------------------------------------------------

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    chip.element.click();
    check("clicking the chip opens the dropdown", menuEl() !== null);
    const items = menuItems();
    check(
      "the dropdown lists the four modes in order",
      items.length === 4 &&
        items[0]?.textContent === "Agent" &&
        items[1]?.textContent === "Ask" &&
        items[2]?.textContent === "Plan" &&
        items[3]?.textContent === "Debug",
    );
    check(
      "every mode item renders its icon",
      items.every((item) => item.querySelector(".workshop-dropdown__icon svg") !== null),
    );
    check(
      "the trigger gains the menu's aria wiring",
      chip.element.getAttribute("aria-haspopup") === "menu" &&
        chip.element.getAttribute("aria-expanded") === "true",
    );
    chip.dispose();
    chip.element.remove();
  }

  // --- Selection -----------------------------------------------------------------

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    const events = [];
    const onModeChanged = (event) => events.push(event);
    document.addEventListener(AGENT_MODE_CHANGED_EVENT, onModeChanged);

    const agentIconHtml = chip.element.querySelector(".mode-chip__icon")?.innerHTML;
    chip.element.click();
    menuItems()[2]?.click();
    check(
      "selecting a mode changes the chip label",
      chip.element.querySelector(".mode-chip__label")?.textContent === "Plan",
    );
    check(
      "selecting a mode changes the chip icon",
      chip.element.querySelector(".mode-chip__icon svg") !== null &&
        chip.element.querySelector(".mode-chip__icon")?.innerHTML !== agentIconHtml,
    );
    check("selecting a mode updates the chip's mode", chip.mode === "plan");
    check(
      "selecting a mode fires agent-mode-changed once with the mode as detail",
      events.length === 1 && events[0]?.detail === "plan",
    );
    check("selecting a mode closes the dropdown", menuEl() === null);

    chip.element.click();
    menuItems()[2]?.click();
    check("re-selecting the current mode fires no event", events.length === 1);

    document.removeEventListener(AGENT_MODE_CHANGED_EVENT, onModeChanged);
    chip.dispose();
    chip.element.remove();
  }

  // --- Dispose ---------------------------------------------------------------------

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    chip.element.click();
    check("a menu is open before dispose", menuEl() !== null);
    chip.dispose();
    check("dispose closes the open menu", menuEl() === null);
    check(
      "dispose restores the trigger's aria state",
      chip.element.getAttribute("aria-expanded") === null,
    );
    chip.element.click();
    check("a disposed chip does not reopen its menu", menuEl() === null);
    chip.element.remove();
  }
});

if (failures.length > 0) {
  console.error(`mode-chip: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("mode-chip: all assertions passed");
process.exit(0);
