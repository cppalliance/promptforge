// The mode chip (src/parts/agent/mode-chip.ts) in jsdom: a button showing the
// current mode's codicon, label, and chevron, with the tooltip "Switch Agent
// Mode (Ctrl+.)". Clicking opens a DropdownMenu of Cursor's five modes on the
// composer's menu surface: a description under each label, a check-only
// selection, opening above the chip at its left edge minus 6px. Picking one
// updates the chip and fires "agent-mode-changed" on document with the mode
// as detail, and onDidChangeMode; re-picking the current mode fires nothing;
// openOrCycle (Ctrl+. and Shift+Tab) opens the menu and, while it is open,
// cycles to the next mode; dispose() closes an open menu. The stylesheet's
// side - the 20px chip, the tints, hiding the label below 300px of composer -
// is read from the source text. Runs under the shared leak check: a ModeChip
// left undisposed fails.
// Run: node test/mode-chip.mjs
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
      export { AGENT_MODE_CHANGED_EVENT, ModeChip, UNIFIED_MODES } from "./src/parts/agent/mode-chip.ts";
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

// The chip reads the DOM globals when it builds, so the jsdom globals must
// exist before the bundle is imported.
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
  return document.querySelector(".menu-popup");
}

function menuItems() {
  return [...(menuEl()?.querySelectorAll(".menu-item") ?? [])];
}

await assertNoLeaks(lifecycle, async () => {
  // --- The mode constants ----------------------------------------------------

  check(
    "the modes are the as-const map the plan fixes",
    UNIFIED_MODES.Agent === "agent" &&
      UNIFIED_MODES.Plan === "plan" &&
      UNIFIED_MODES.Debug === "debug" &&
      UNIFIED_MODES.Multitask === "multitask" &&
      UNIFIED_MODES.Ask === "ask",
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
      "the chip has the ws-mode-chip class",
      chip.element.classList.contains("ws-mode-chip"),
    );
    check("the chip starts on the agent mode", chip.mode === "agent");
    check(
      "the chip shows the current mode's label and icon",
      chip.element.querySelector(".ws-mode-chip__label")?.textContent === "Agent" &&
        chip.element.querySelector(".ws-mode-chip__icon svg") !== null &&
        chip.element.querySelector(".ws-mode-chip__chevron svg") !== null,
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
    const labels = items.map(
      (item) => item.querySelector(".menu-item__label")?.textContent,
    );
    check(
      "the dropdown lists Cursor's five modes in order",
      items.length === 5 &&
        labels.join(",") === "Agent,Plan,Debug,Multitask,Ask",
    );
    check(
      "the current mode row shows the codicon check, and only that row",
      items[0]?.querySelector(".menu-item__check svg") !== null &&
        items.slice(1).every((item) => item.querySelector(".menu-item__check") === null),
    );
    check(
      "every mode item renders its icon",
      items.every((item) => item.querySelector(".menu-item__icon svg") !== null),
    );
    check(
      "every mode row carries Cursor's description under its label",
      items
        .map((item) => item.querySelector(".menu-item__description")?.textContent)
        .join("|") ===
        [
          "Plan, search, build anything",
          "Create detailed plans for accomplishing tasks",
          "Systematically diagnose and fix bugs using runtime traces",
          "Run and coordinate multiple tasks in parallel",
          "Ask Cursor questions about your codebase",
        ].join("|"),
    );
    check(
      "the menu wears the composer surface, sized by the mode menu class",
      menuEl()?.classList.contains("menu-composer") === true &&
        menuEl()?.classList.contains("ws-mode-menu") === true,
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

    const agentIconHtml = chip.element.querySelector(".ws-mode-chip__icon")?.innerHTML;
    chip.element.click();
    menuItems()[1]?.click();
    check(
      "selecting a mode changes the chip label",
      chip.element.querySelector(".ws-mode-chip__label")?.textContent === "Plan",
    );
    check(
      "selecting a mode changes the chip icon",
      chip.element.querySelector(".ws-mode-chip__icon svg") !== null &&
        chip.element.querySelector(".ws-mode-chip__icon")?.innerHTML !== agentIconHtml,
    );
    check("selecting a mode updates the chip's mode", chip.mode === "plan");
    check(
      "selecting a mode fires agent-mode-changed once with the mode as detail",
      events.length === 1 && events[0]?.detail === "plan",
    );
    check("selecting a mode closes the dropdown", menuEl() === null);

    chip.element.click();
    menuItems()[1]?.click();
    check("re-selecting the current mode fires no event", events.length === 1);

    document.removeEventListener(AGENT_MODE_CHANGED_EVENT, onModeChanged);
    chip.dispose();
    chip.element.remove();
  }

  // --- The tooltip, the placement, and the change event ---------------------------

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    check(
      "the chip's tooltip is Switch Agent Mode (Ctrl+.)",
      chip.element.title === "Switch Agent Mode (Ctrl+.)",
    );
    chip.element.getBoundingClientRect = () => ({
      x: 40, y: 300, left: 40, right: 100, top: 300, bottom: 320, width: 60, height: 20, toJSON: () => ({}),
    });
    chip.element.click();
    check(
      "the menu opens at the chip's left edge minus 6px",
      menuEl()?.style.left === "34px",
    );
    check(
      "the menu opens above the chip",
      Number.parseFloat(menuEl()?.style.top ?? "9999") < 300,
    );
    chip.dispose();
    chip.element.remove();
  }

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    const seen = [];
    const subscription = chip.onDidChangeMode((mode) => seen.push(mode));
    chip.element.click();
    menuItems()[4]?.click();
    check("onDidChangeMode fires with the picked mode", seen.join(",") === "ask" && chip.mode === "ask");
    check(
      "the chip carries its mode as data-mode for the tint",
      chip.element.dataset.mode === "ask",
    );
    chip.element.click();
    menuItems()[4]?.click();
    check("re-picking the current mode fires no change event", seen.length === 1);
    subscription.dispose();
    chip.dispose();
    chip.element.remove();
  }

  // --- openOrCycle: Ctrl+. and Shift+Tab ------------------------------------------------

  {
    const chip = new ModeChip();
    document.body.appendChild(chip.element);
    const seen = [];
    chip.onDidChangeMode((mode) => seen.push(mode));
    chip.openOrCycle();
    check("the first press opens the menu without changing the mode", menuEl() !== null && seen.length === 0);
    chip.openOrCycle();
    check(
      "pressing again cycles to the next mode and keeps the menu open on it",
      chip.mode === "plan" &&
        menuEl() !== null &&
        menuItems()[1]?.querySelector(".menu-item__check") !== null &&
        menuItems()[0]?.querySelector(".menu-item__check") === null,
    );
    chip.openOrCycle();
    chip.openOrCycle();
    chip.openOrCycle();
    check("cycling walks Debug, Multitask, Ask in menu order", seen.join(",") === "plan,debug,multitask,ask");
    chip.openOrCycle();
    check("cycling wraps from Ask back to Agent", chip.mode === "agent" && seen.at(-1) === "agent");

    // Shift+Tab with the menu open and focus in it: the document listener cycles
    // instead of the menu treating Tab as its dismissal.
    const before = chip.mode;
    menuEl()?.dispatchEvent(
      new dom.window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true }),
    );
    check(
      "Shift+Tab in the open menu cycles the mode and leaves the menu open",
      chip.mode !== before && menuEl() !== null,
    );
    chip.dispose();
    chip.element.remove();
  }

  // dispose() closes the menu before anything else, and the Shift+Tab handler
  // acts only while the menu is open, so a keypress after dispose cannot tell a
  // removed listener from a leaked one. Watch the document's registrations
  // directly: every capture keydown listener a chip adds must be the one its
  // dispose removes.
  {
    const added = [];
    const removed = [];
    const realAdd = document.addEventListener;
    const realRemove = document.removeEventListener;
    document.addEventListener = function addEventListener(type, listener, options) {
      added.push({ type, listener, options });
      return realAdd.call(this, type, listener, options);
    };
    document.removeEventListener = function removeEventListener(type, listener, options) {
      removed.push({ type, listener, options });
      return realRemove.call(this, type, listener, options);
    };
    let chip;
    try {
      chip = new ModeChip();
      document.body.appendChild(chip.element);
      const keydownBeforeDispose = added.filter((entry) => entry.type === "keydown");
      check(
        "a chip listens for Shift+Tab with one capture keydown listener on the document",
        keydownBeforeDispose.length === 1 && keydownBeforeDispose[0]?.options === true,
      );
      check("nothing is removed while the chip lives", removed.length === 0);
      chip.dispose();
    } finally {
      document.addEventListener = realAdd;
      document.removeEventListener = realRemove;
    }
    const registered = added.find((entry) => entry.type === "keydown");
    check(
      "dispose removes that same keydown listener with the same capture flag",
      registered !== undefined &&
        removed.some(
          (entry) =>
            entry.type === "keydown" && entry.listener === registered.listener && entry.options === registered.options,
        ),
    );
    chip.element.remove();
  }

  // --- The stylesheet --------------------------------------------------------------------

  {
    const css = (
      await readFile(path.join(testDir, "..", "src", "parts", "agent", "mode-chip.css"), "utf8")
    ).replace(/\/\*[\s\S]*?\*\//g, "");
    const rule = (selector) => {
      const match = new RegExp(`${selector.replace(/[.[\]"=]/g, "\\$&")}\\s*\\{([^}]*)\\}`).exec(css);
      return match === null ? "" : match[1].replace(/\s+/g, " ");
    };
    check("the chip is 20px tall", /block-size:\s*var\(--height-xs\)/.test(rule(".ws-mode-chip")));
    check(
      "the chip pads 2px 4px 2px 8px",
      /padding:\s*var\(--ws-size-2\) var\(--space-1\) var\(--ws-size-2\) var\(--space-2\)/.test(rule(".ws-mode-chip")),
    );
    check(
      "the icon is 16px at 0.5 opacity",
      /\.ws-mode-chip__icon,\s*\.ws-mode-chip__chevron\s*\{[^}]*opacity:\s*0\.5/.test(css) &&
        /inline-size:\s*var\(--ws-size-16\)/.test(rule(".ws-mode-chip__icon svg")),
    );
    check("the label reads at 0.8", /opacity:\s*0\.8/.test(rule(".ws-mode-chip__label")));
    check("the chevron is 14px", /inline-size:\s*var\(--ws-size-14\)/.test(rule(".ws-mode-chip__chevron svg")));
    check(
      "Plan tints yellow, Ask green, Debug red, Multitask violet, each a 24% fill under text of the same color",
      [
        ["plan", "yellow"],
        ["ask", "green"],
        ["debug", "red"],
        ["multitask", "multitask"],
      ].every(([mode, color]) => {
        const body = rule(`.ws-mode-chip[data-mode="${mode}"]`);
        return body.includes(`color: var(--cursor-${color})`) && body.includes(`var(--cursor-${color}) 24%`);
      }),
    );
    check(
      "the label hides below 300px of composer",
      /@container ws-composer \(width < 300px\)\s*\{\s*\.ws-mode-chip__label\s*\{\s*display:\s*none/.test(css),
    );
    check(
      "the menu is at least 170px wide",
      /min-inline-size:\s*var\(--ws-mode-menu-min-width\)/.test(rule(".ws-mode-menu")),
    );
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
  console.error(`ws-mode-chip: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ws-mode-chip: all assertions passed");
process.exit(0);
