// Unit test for the quick input widget (src/ui/quickinput/quick-input.ts):
// the floating panel under the title bar with the WAI-ARIA combobox
// pattern (a role=combobox input with aria-autocomplete, aria-expanded,
// aria-controls, and aria-activedescendant, a visually-hidden label, and
// a ul role=listbox of li role=option rows), longest-prefix re-routing
// in place as the input changes (the filter handed to a provider is the
// value minus the prefix), ArrowUp/ArrowDown moving the active option,
// Enter accepting the active row and closing, Escape closing with focus
// restored, a factory whose product is not a provider rendering an empty
// list, and includeHelp rendering every provider's help entries above
// the active provider's rows with a help row re-routing to its prefix.
// Bundles the module with esbuild and drives it against jsdom.
// Run: node --test test/quick-input.mjs
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
globalThis.HTMLInputElement = window.HTMLInputElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { QuickInputService } from "./src/ui/quickinput/quick-input.ts";
      export { createQuickAccessRegistry } from "./src/services/quick-access-registry.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  loader: { ".css": "empty" },
});
const { QuickInputService, createQuickAccessRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Fixture: a registry with stub providers at four prefixes -----------------

const registry = createQuickAccessRegistry();
const accepted = [];

function stubProvider(prefix, placeholder, helpEntries, labels) {
  const filters = [];
  registry.registerQuickAccessProvider({
    prefix,
    placeholder,
    helpEntries,
    factory: () => ({
      getItems(filter) {
        filters.push(filter);
        return labels.map((label) => ({
          label,
          accept: () => accepted.push([prefix, label]),
        }));
      },
    }),
  });
  return filters;
}

const fileFilters = stubProvider("", "Search files by name", [{ description: "Go to File", prefix: "" }], [
  "a.ts",
  "b.ts",
]);
const commandFilters = stubProvider(">", "Type a command", [{ description: "Show and Run Commands", prefix: ">" }], [
  "Save",
  "Save All",
]);
const debugFilters = stubProvider("debug ", "Debug configurations", [{ description: "Start Debugging", prefix: "debug " }], [
  "Launch",
]);
registry.registerQuickAccessProvider({
  prefix: "%",
  placeholder: "Not a provider",
  helpEntries: [],
  factory: () => ({}),
});

const quickInput = new QuickInputService({ registry });

function panel() {
  return window.document.querySelector(".ws-quick-input");
}
function input() {
  return panel().querySelector("input");
}
function options() {
  return [...panel().querySelectorAll('[role="option"]')];
}
function optionLabels() {
  return options().map((row) => row.querySelector(".ws-quick-input__option-label")?.textContent);
}
function type(value) {
  input().value = value;
  input().dispatchEvent(new window.Event("input", { bubbles: true }));
}
function key(name) {
  input().dispatchEvent(new window.KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
}

// --- Closed until shown; show renders the combobox ------------------------------

check("the panel starts hidden", panel().hidden === true);

quickInput.quickAccess.show("");

{
  check("show reveals the panel", panel().hidden === false);
  check("the input holds focus", window.document.activeElement === input());
  check("the input is a combobox", input().getAttribute("role") === "combobox");
  check("the combobox autocompletes from its list", input().getAttribute("aria-autocomplete") === "list");
  check("the combobox reports expanded", input().getAttribute("aria-expanded") === "true");
  const list = panel().querySelector("ul");
  check("the list is a listbox", list?.getAttribute("role") === "listbox");
  check("aria-controls names the listbox", input().getAttribute("aria-controls") === list?.id);
  const label = panel().querySelector("label");
  check("a visually-hidden label names the input", label?.textContent.length > 0);
  check("the placeholder comes from the routed provider", input().placeholder === "Search files by name");
  check("the rows come from the empty-prefix provider", optionLabels().join(",") === "a.ts,b.ts");
  check("every row is an option", options().every((row) => row.tagName === "LI"));
  check(
    "the first row starts active",
    input().getAttribute("aria-activedescendant") === options()[0]?.id &&
      options()[0]?.getAttribute("aria-selected") === "true",
  );
}

// --- Enter accepts the active row and closes ------------------------------------

key("Enter");
check("Enter runs the first row's accept", accepted.length === 1 && accepted[0][1] === "a.ts");
check("Enter closes the panel", panel().hidden === true);
check("a closed combobox reports collapsed", input().getAttribute("aria-expanded") === "false");

// --- Arrow keys move the active row; Enter accepts it ----------------------------

quickInput.quickAccess.show("");
key("ArrowDown");
check(
  "ArrowDown moves the active descendant",
  input().getAttribute("aria-activedescendant") === options()[1]?.id,
);
key("ArrowDown");
check("ArrowDown wraps past the last row", input().getAttribute("aria-activedescendant") === options()[0]?.id);
key("ArrowUp");
check("ArrowUp wraps before the first row", input().getAttribute("aria-activedescendant") === options()[1]?.id);
key("Enter");
check("Enter accepts the moved-to row", accepted.length === 2 && accepted[1][1] === "b.ts");

// --- Prefix routing strips the prefix and re-routes in place ---------------------

quickInput.quickAccess.show(">sa");
check("the routed provider's placeholder shows", input().placeholder === "Type a command");
check("the provider sees the value minus its prefix", commandFilters.at(-1) === "sa");
check("the routed provider's rows render", optionLabels().join(",") === "Save,Save All");

type("");
check("clearing the input re-routes to the default provider", input().placeholder === "Search files by name");
type("debug foo");
check("the longest matching prefix wins", input().placeholder === "Debug configurations");
check("the debug provider sees its filter", debugFilters.at(-1) === "foo");
check("the panel never closed during re-routing", panel().hidden === false);
check("the default provider saw its empty filter", fileFilters.includes(""));
key("Escape");

// --- Escape closes and returns focus ---------------------------------------------

const before = window.document.createElement("button");
before.type = "button";
window.document.body.appendChild(before);
before.focus();
quickInput.quickAccess.show("");
key("Escape");
check("Escape closes the panel", panel().hidden === true);
check("Escape returns focus to the prior element", window.document.activeElement === before);

// --- A factory whose product is not a provider renders an empty list -------------

quickInput.quickAccess.show("%");
check("a non-provider factory renders no rows", options().length === 0);
check("a non-provider factory keeps its placeholder", input().placeholder === "Not a provider");
check("an empty list clears the active descendant", input().getAttribute("aria-activedescendant") === null);
key("Enter");
check("Enter with no rows accepts nothing", accepted.length === 2);
check("Enter with no rows leaves the panel open", panel().hidden === false);
key("Escape");

// --- includeHelp renders the modes list above the provider's rows -----------------

quickInput.quickAccess.show("", { includeHelp: true });
{
  check(
    "help entries render above the provider's rows",
    optionLabels().join(",") === "Go to File,Show and Run Commands,Start Debugging,a.ts,b.ts",
  );
  key("ArrowDown");
  key("Enter");
  check("accepting a help row re-routes to its prefix", input().value === ">");
  check("a help row keeps the panel open", panel().hidden === false);
  check("the re-routed provider's rows render", optionLabels().join(",") === "Save,Save All");
  key("Escape");
}

quickInput.dispose();
check("dispose removes the panel", window.document.querySelector(".ws-quick-input") === null);

if (failures.length > 0) {
  console.error(`quick-input: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("quick-input: all assertions passed");
