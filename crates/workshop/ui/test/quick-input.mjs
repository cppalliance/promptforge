// Unit test for the quick input widget (src/parts/quickinput/quick-input.ts):
// the floating panel under the title bar with the WAI-ARIA combobox
// pattern (a role=combobox input with aria-autocomplete, aria-expanded,
// aria-controls, and aria-activedescendant, a visually-hidden label, and
// a ul role=listbox of li role=option rows), longest-prefix re-routing
// in place as the input changes (the filter handed to a provider is the
// value minus the prefix), ArrowUp/ArrowDown moving the active option,
// Enter accepting the active row and closing, Escape closing with focus
// restored, a factory whose product is not a provider rendering an empty
// list, and includeHelp rendering the help entries that carry a
// commandCenterOrder above the active provider's rows (sorted by it,
// relabelled by commandCenterLabel, with the prefix as the description and
// the command's keybinding as chips) with a row re-routing to its provider's
// prefix. Rows draw key chips (one per key, "+" between, a gap between
// chords), match highlights, group labels above the row they open, and an
// empty message for a provider that has one.
// The provider half covers the command palette provider (Category: Title
// labels, keybinding labels, precondition filtering, substring filter and
// its highlights, the "recently used" and "other commands" group labels,
// "No matching commands", CommandsHistory recency over the fake UI-state
// adapter, the COMMANDS_HISTORY registry token the provider resolves when
// no history is injected), the ? help provider (prefix labels, help-text
// descriptions, sorted by prefix, ? itself left out, the ?> jump), the
// not-available placeholder providers, and the real provider descriptors'
// Cursor wording and modes list rendering before the recent files.
// Bundles the module with esbuild and drives it against jsdom.
// Run: node --test test/quick-input.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { createFakeUiStorage } from "./helpers/ui-storage.mjs";

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
      export { QuickInputService } from "./src/parts/quickinput/quick-input.ts";
      export { createQuickAccessRegistry } from "@workshop/platform/quick-access-registry";
      export { CommandsHistory } from "./src/parts/quickinput/commands-history.ts";
      export { COMMANDS_HISTORY } from "./src/services/commands-history.ts";
      export { getService, registerService } from "@workshop/platform/service-registry";
      export {
        createCommandPaletteProvider,
        createHelpProvider,
        createPlaceholderProvider,
        createQuickAccessProviderDescriptors,
      } from "./src/parts/quickinput/quick-access-providers.ts";
      export { CommandRegistry } from "@workshop/platform/command-registry";
      export { MenuRegistry, MenuId } from "@workshop/platform/menu-registry";
      export { createKeybindingsRegistry } from "@workshop/platform/keybinding-registry";
      export { ContextKeyService } from "@workshop/platform/context-key-service";
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
const {
  QuickInputService,
  createQuickAccessRegistry,
  CommandsHistory,
  COMMANDS_HISTORY,
  getService,
  registerService,
  createCommandPaletteProvider,
  createHelpProvider,
  createPlaceholderProvider,
  createQuickAccessProviderDescriptors,
  CommandRegistry,
  MenuRegistry,
  MenuId,
  createKeybindingsRegistry,
  ContextKeyService,
} = await import(
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

// Only entries with a commandCenterOrder reach the modes list, sorted by it:
// the file entry first, then the debug entry (relabelled), then commands. The
// entry without an order never shows there.
const fileFilters = stubProvider(
  "",
  "Search files by name",
  [{ description: "Go to File", commandId: "test.quickOpen", commandCenterOrder: 10 }],
  ["a.ts", "b.ts"],
);
const commandFilters = stubProvider(
  ">",
  "Type a command",
  [{ description: "Show and Run Commands", prefix: ">", commandCenterOrder: 60 }],
  ["Save", "Save All"],
);
const debugFilters = stubProvider(
  "debug ",
  "Debug configurations",
  [
    { description: "Start Debugging", prefix: "debug ", commandCenterOrder: 20, commandCenterLabel: "Debug" },
    { description: "Never in the modes list", prefix: "debug " },
  ],
  ["Launch"],
);
registry.registerQuickAccessProvider({
  prefix: "%",
  placeholder: "Not a provider",
  helpEntries: [],
  factory: () => ({}),
});

const quickKeybindings = createKeybindingsRegistry("linux");
quickKeybindings.registerKeybindingRule({ id: "test.quickOpen", keybinding: "ctrl+p" });
const quickInput = new QuickInputService({ registry, keybindings: quickKeybindings });

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
    "ordered help entries render above the provider's rows, relabelled and sorted",
    optionLabels().join(",") === "Go to File,Debug,Show and Run Commands,a.ts,b.ts",
  );
  const descriptions = options().map((row) => row.querySelector(".ws-quick-input__option-description")?.textContent);
  check(
    "a mode row shows its prefix as the description, and the default mode shows none",
    descriptions[0] === undefined && descriptions[1] === "debug " && descriptions[2] === ">",
  );
  const chips = [...options()[0].querySelectorAll(".ws-quick-input__key")].map((chip) => chip.textContent);
  check("a mode row shows its command's keybinding as chips", chips.join(",") === "Ctrl,P");
  key("ArrowDown");
  key("Enter");
  check("accepting a mode row re-routes to its provider's prefix", input().value === "debug ");
  check("a mode row keeps the panel open", panel().hidden === false);
  check("the re-routed provider's rows render", optionLabels().join(",") === "Launch");
  key("Escape");
}

// The panels below find theirs by class, so the shared one goes first.
quickInput.dispose();
check("dispose removes the panel", window.document.querySelector(".ws-quick-input") === null);

// --- Rows: key chips, match highlights, group labels, the empty message -----------

{
  const rowsRegistry = createQuickAccessRegistry();
  let rowItems = [];
  rowsRegistry.registerQuickAccessProvider({
    prefix: "",
    placeholder: "rows",
    helpEntries: [],
    factory: () => ({ getItems: () => rowItems, noResultsMessage: "No matching commands" }),
  });
  const rowsInput = new QuickInputService({ registry: rowsRegistry });
  const accept = () => {};
  rowItems = [
    {
      label: "File: Save",
      labelHighlights: [{ start: 6, end: 10 }],
      keybinding: "Ctrl+Shift+P",
      separator: "recently used",
      accept,
    },
    { label: "File: Save All", description: "every file", keybinding: "Ctrl+K Ctrl+S", separator: "other commands", accept },
    { label: "Plain", accept },
  ];
  rowsInput.quickAccess.show("");
  const first = options()[0];
  const second = options()[1];
  check(
    "a match highlight wraps only the matched text",
    [...first.querySelectorAll(".ws-quick-input__highlight")].map((mark) => mark.textContent).join("|") === "Save",
  );
  check(
    "a highlighted label still reads as one label",
    first.querySelector(".ws-quick-input__option-label")?.textContent === "File: Save",
  );
  check(
    "a plain row has no highlight",
    options()[2].querySelector(".ws-quick-input__highlight") === null,
  );
  check(
    "a keybinding renders one chip per key",
    [...first.querySelectorAll(".ws-quick-input__key")].map((chip) => chip.textContent).join(",") === "Ctrl,Shift,P",
  );
  check(
    "a plus sits between the keys of a chord",
    [...first.querySelectorAll(".ws-quick-input__key-separator")].map((plus) => plus.textContent).join("") === "++",
  );
  check(
    "a two-chord keybinding renders four chips and a chord separator",
    [...second.querySelectorAll(".ws-quick-input__key")].map((chip) => chip.textContent).join(",") === "Ctrl,K,Ctrl,S" &&
      second.querySelectorAll(".ws-quick-input__chord-separator").length === 1,
  );
  check("a row without a keybinding has no chips", options()[2].querySelector(".ws-quick-input__key") === null);
  const separators = [...panel().querySelectorAll(".ws-quick-input__separator")];
  check(
    "each group label sits above its row",
    separators.map((label) => label.textContent).join(",") === "recently used,other commands" &&
      separators[0].nextElementSibling === first &&
      separators[1].nextElementSibling === second,
  );
  check("a group label is not an option", options().length === 3 && separators.every((label) => label.getAttribute("role") === "presentation"));
  key("ArrowDown");
  check(
    "arrow keys step over a group label to the next row",
    input().getAttribute("aria-activedescendant") === second.id,
  );
  check("no empty message while rows exist", panel().querySelector(".ws-quick-input__empty") === null);

  rowItems = [];
  type("zzz");
  const empty = panel().querySelector(".ws-quick-input__empty");
  check("a provider with no rows shows its empty message", empty?.textContent === "No matching commands");
  check("the empty message is not an option", options().length === 0 && empty?.getAttribute("role") === "presentation");
  key("Enter");
  check("Enter on the empty message does nothing", panel().hidden === false);
  key("Escape");
  rowsInput.dispose();
}

{
  // A provider may answer several spans, in any order, and a bad one never
  // breaks the label: the spans draw in label order, a span that overlaps
  // the one before it, runs off the label, or is empty is skipped, and the
  // label's text always reads whole.
  const label = "File: Save All";
  const spanRegistry = createQuickAccessRegistry();
  let spans = [];
  spanRegistry.registerQuickAccessProvider({
    prefix: "",
    placeholder: "spans",
    helpEntries: [],
    factory: () => ({ getItems: () => [{ label, labelHighlights: spans, accept() {} }] }),
  });
  const spanInput = new QuickInputService({ registry: spanRegistry });
  const marks = () =>
    [...options()[0].querySelectorAll(".ws-quick-input__highlight")].map((mark) => mark.textContent).join("|");
  const reads = () => options()[0].querySelector(".ws-quick-input__option-label")?.textContent === label;
  const render = (next) => {
    spans = next;
    spanInput.quickAccess.show("");
    return marks();
  };

  check(
    "two spans given out of order draw in label order",
    render([
      { start: 11, end: 14 },
      { start: 0, end: 4 },
    ]) === "File|All" && reads(),
  );
  key("Escape");
  check(
    "a span that overlaps the one before it is skipped",
    render([
      { start: 0, end: 6 },
      { start: 4, end: 8 },
    ]) === "File: " && reads(),
  );
  key("Escape");
  check("a span that runs off the label is skipped", render([{ start: 11, end: 99 }]) === "" && reads());
  key("Escape");
  check(
    "a valid span still draws beside a skipped out-of-range one",
    render([
      { start: 20, end: 30 },
      { start: 6, end: 10 },
    ]) === "Save" && reads(),
  );
  key("Escape");
  check(
    "an empty or inverted span is skipped",
    render([
      { start: 3, end: 3 },
      { start: 9, end: 5 },
    ]) === "" && reads(),
  );
  key("Escape");
  spanInput.dispose();
}

{
  // A provider without a message shows nothing when it has no rows.
  const plainRegistry = createQuickAccessRegistry();
  plainRegistry.registerQuickAccessProvider({
    prefix: "",
    placeholder: "plain",
    helpEntries: [],
    factory: () => ({ getItems: () => [] }),
  });
  const plainInput = new QuickInputService({ registry: plainRegistry });
  plainInput.quickAccess.show("");
  check("a provider without a message shows no empty row", panel().querySelector(".ws-quick-input__empty") === null);
  key("Escape");
  plainInput.dispose();
}

// --- Provider half: the command palette provider --------------------------------

/** The user-bucket key the composition root binds the history to. */
const HISTORY_KEY = "commands_history";

/**
 * Builds a history over a fake adapter the way main.ts binds the live
 * one: the initial value read from the user bucket, every change written
 * back to the same key. Returns the history and the fake, whose `sets`
 * records the writes.
 */
function historyOver(initial) {
  const storage = createFakeUiStorage(initial === undefined ? {} : { user: { [HISTORY_KEY]: initial } });
  const history = new CommandsHistory(storage.get("user", HISTORY_KEY), (value) =>
    storage.set("user", HISTORY_KEY, value),
  );
  return { history, storage };
}

function paletteSetup(initialHistory) {
  const commands = new CommandRegistry();
  const menus = new MenuRegistry();
  const keybindings = createKeybindingsRegistry("windows");
  const context = new ContextKeyService();
  const { history, storage } = historyOver(initialHistory);
  const provider = createCommandPaletteProvider({ commands, menus, keybindings, context, history });
  return { commands, menus, keybindings, context, storage, history, provider };
}

{
  // Palette rows are Category: Title and show the keybinding label.
  const { commands, menus, keybindings, provider } = paletteSetup();
  let ran = 0;
  commands.register("file.save", { title: "Save", category: "File", run: () => { ran += 1; } });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "file.save" });
  keybindings.registerKeybindingRule({ id: "file.save", keybinding: "ctrl+s" });
  const rows = provider.getItems("");
  check("the palette lists the palette-menu command", rows.length === 1);
  check("the palette label is Category: Title", rows[0]?.label === "File: Save");
  check("the palette row shows the keybinding label", rows[0]?.keybinding === "Ctrl+S");
  check("with no history the palette shows no group label", rows[0]?.separator === undefined);
  check("the palette's empty result reads 'No matching commands'", provider.noResultsMessage === "No matching commands");
  rows[0]?.accept();
  await Promise.resolve();
  check("accept runs the command through the registry", ran === 1);
}

{
  // A command whose precondition fails is absent from the palette.
  const { commands, menus, context, provider } = paletteSetup();
  const focus = context.createKey("editorTextFocus", false);
  commands.register("editor.format", { title: "Format", precondition: "editorTextFocus", run: () => {} });
  commands.register("file.save", { title: "Save", category: "File", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "editor.format" });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "file.save" });
  check(
    "a failed precondition keeps the command out of the palette",
    provider.getItems("").map((row) => row.label).join(",") === "File: Save",
  );
  focus.set(true);
  check("a met precondition admits the command", provider.getItems("").length === 2);
  context.dispose();
}

{
  // The filter narrows rows; a category-less command labels with its title.
  const { commands, menus, provider } = paletteSetup();
  commands.register("file.save", { title: "Save", category: "File", run: () => {} });
  commands.register("window.reload", { title: "Reload Window", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "file.save" });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "window.reload" });
  check("a category-less label is the bare title", provider.getItems("").map((row) => row.label).join(",") === "File: Save,Reload Window");
  check("the filter narrows case-insensitively", provider.getItems("reload").map((row) => row.label).join(",") === "Reload Window");
  check("a filter matching nothing renders no rows", provider.getItems("zzz").length === 0);
  const reload = provider.getItems("rEl")[0];
  check(
    "the matched text of the label is highlighted",
    reload?.labelHighlights?.length === 1 && reload.labelHighlights[0].start === 0 && reload.labelHighlights[0].end === 3,
  );
  const save = provider.getItems("sav")[0];
  check(
    "a match inside the title is highlighted past its category",
    save?.labelHighlights?.[0]?.start === 6 && save.labelHighlights[0].end === 9,
  );
  check("an empty filter highlights nothing", provider.getItems("").every((row) => row.labelHighlights === undefined));
}

{
  // Recently used commands sort first, in recency order; accept records history.
  const { commands, menus, storage, history, provider } = paletteSetup();
  commands.register("a.one", { title: "One", run: () => {} });
  commands.register("b.two", { title: "Two", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "a.one" });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "b.two" });
  check("without history the rows keep menu order", provider.getItems("").map((row) => row.label).join(",") === "One,Two");
  check("without history no row carries a group label", provider.getItems("").every((row) => row.separator === undefined));
  history.add("b.two");
  check("a used command sorts first", provider.getItems("").map((row) => row.label).join(",") === "Two,One");
  check(
    "the used commands open under 'recently used' and the rest under 'other commands'",
    provider.getItems("")[0]?.separator === "recently used" && provider.getItems("")[1]?.separator === "other commands",
  );
  provider.getItems("")[1]?.accept();
  await Promise.resolve();
  check("accept records the command in the history", history.list[0] === "a.one");
  check(
    "each add writes the full id list to the user bucket",
    storage.sets.length === 2 &&
      storage.sets.every((entry) => entry.bucket === "user" && entry.key === HISTORY_KEY) &&
      storage.sets[0].value.join(",") === "b.two" &&
      storage.sets[1].value.join(",") === "a.one,b.two",
  );
  const reloaded = new CommandsHistory(storage.get("user", HISTORY_KEY), () => {});
  check("a new history over the written value reads it back", reloaded.list.join(",") === "a.one,b.two");
  check("a malformed initial value reads as no history", historyOver({ not: "a list" }).history.list.length === 0);
  check("a string initial value reads as no history, never a cast", historyOver('["x"]').history.list.length === 0);
  check("a null initial value reads as no history", historyOver(null).history.list.length === 0);
  check(
    "non-string, empty, and duplicate initial entries drop out",
    historyOver(["ok", 7, "", "ok", "two"]).history.list.join(",") === "ok,two",
  );
}

{
  // The initial list drives recency before any command runs; construction writes nothing.
  const { commands, menus, storage, provider } = paletteSetup(["b.two"]);
  commands.register("a.one", { title: "One", run: () => {} });
  commands.register("b.two", { title: "Two", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "a.one" });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "b.two" });
  check("the initial history list sorts its command first", provider.getItems("").map((row) => row.label).join(",") === "Two,One");
  check("construction writes nothing", storage.sets.length === 0);
}

{
  // Only recent commands: one 'recently used' label and no 'other commands'.
  const { commands, menus, provider } = paletteSetup(["a.one"]);
  commands.register("a.one", { title: "One", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "a.one" });
  const rows = provider.getItems("");
  check(
    "a palette of only recent commands has the one label",
    rows.length === 1 && rows[0].separator === "recently used",
  );
}

{
  // The cap, the empty id, and a writer that throws.
  const { history, storage } = historyOver();
  history.add("");
  check("an empty id is ignored and writes nothing", history.list.length === 0 && storage.sets.length === 0);
  for (let i = 0; i < 55; i += 1) {
    history.add(`cmd.${i}`);
  }
  check("the history is capped at 50", history.list.length === 50 && history.list[0] === "cmd.54");
  check("the written list is the capped list", storage.sets[storage.sets.length - 1].value.length === 50);

  let attempts = 0;
  const degraded = new CommandsHistory(["kept"], () => {
    attempts += 1;
    throw new Error("denied");
  });
  degraded.add("fresh");
  check("a throwing writer is still called", attempts === 1);
  check("a rejected write leaves the in-memory list intact", degraded.list.join(",") === "fresh,kept");
}

{
  // The registry token: an empty no-op default self-registers, and the
  // palette provider resolves it when no history is injected, so the
  // composition root's re-registration reaches the palette.
  const fallback = getService(COMMANDS_HISTORY);
  check("COMMANDS_HISTORY self-registers a CommandsHistory", fallback instanceof CommandsHistory);
  check("the default history starts empty", fallback.list.length === 0);
  const { history: bound } = historyOver(["b.two"]);
  registerService(COMMANDS_HISTORY, () => bound);
  const commands = new CommandRegistry();
  const menus = new MenuRegistry();
  const context = new ContextKeyService();
  commands.register("a.one", { title: "One", run: () => {} });
  commands.register("b.two", { title: "Two", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "a.one" });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "b.two" });
  const provider = createCommandPaletteProvider({ commands, menus, context });
  check(
    "without an injected history the palette resolves COMMANDS_HISTORY",
    provider.getItems("").map((row) => row.label).join(",") === "Two,One",
  );
  provider.getItems("")[1]?.accept();
  await Promise.resolve();
  check("accept records into the registry-resolved history", bound.list[0] === "a.one");
  context.dispose();
}

// --- Provider half: the help provider --------------------------------------------

/**
 * A registry shaped like Cursor's: the default provider, a symbol provider
 * with two entries, the commands provider, and the ? provider itself. Each
 * stub provider answers one row named after its prefix.
 */
function helpFixture() {
  const helpRegistry = createQuickAccessRegistry();
  const stub = (prefix, helpEntries) =>
    helpRegistry.registerQuickAccessProvider({
      prefix,
      placeholder: `placeholder for "${prefix}"`,
      helpEntries,
      factory: () => ({ getItems: () => [{ label: `row of "${prefix}"`, accept: () => {} }] }),
    });
  stub(">", [{ description: "Show and Run Commands", commandId: "test.showCommands" }]);
  stub("@", [
    { description: "Go to Symbol in Editor", prefix: "@" },
    { description: "Go to Symbol in Editor by Category", prefix: "@:" },
  ]);
  stub("", [{ description: "Go to File", commandId: "test.quickOpen" }]);
  const shown = [];
  const keybindings = createKeybindingsRegistry("linux");
  keybindings.registerKeybindingRule({ id: "test.quickOpen", keybinding: "ctrl+p" });
  const help = createHelpProvider({ registry: helpRegistry, keybindings, show: (value) => shown.push(value) });
  helpRegistry.registerQuickAccessProvider({
    prefix: "?",
    placeholder: "Type '?' to get help",
    helpEntries: [{ description: "Show all Quick Access Providers", commandCenterOrder: 70, commandCenterLabel: "More" }],
    factory: () => help,
  });
  return { helpRegistry, help, shown, keybindings };
}

{
  const { help, shown } = helpFixture();
  const rows = help.getItems("");
  check(
    "a help row's label is its prefix, or an ellipsis for the default mode",
    rows.map((row) => row.label).join(",") === "\u2026,@,@:,>",
  );
  check(
    "a help row's description is the help text",
    rows.map((row) => row.description).join("|") ===
      "Go to File|Go to Symbol in Editor|Go to Symbol in Editor by Category|Show and Run Commands",
  );
  check("the ? provider's own entry is left out", !rows.some((row) => row.label === "?"));
  check("a help row shows its command's keybinding", rows[0]?.keybinding === "Ctrl+P" && rows[3]?.keybinding === undefined);
  rows[3]?.accept();
  rows[2]?.accept();
  check("accepting a help row enters its mode", shown.join(",") === ">,@:");
}

{
  // ?> jumps into the > mode: the help provider redirects the typed value.
  const { help, helpRegistry } = helpFixture();
  check("typing a prefix after ? redirects to that mode", help.redirect?.(">") === ">");
  check("a longer prefix after ? redirects to its provider's prefix", help.redirect?.("@:x") === "@");
  check("a bare ? stays in help", help.redirect?.("") === undefined);
  check("text that routes to the default mode stays in help", help.redirect?.("save") === undefined);
  check("a second ? stays in help", help.redirect?.("?") === undefined);

  const jumpInput = new QuickInputService({ registry: helpRegistry });
  jumpInput.quickAccess.show("?");
  check("the ? list renders in the widget", optionLabels().join(",") === "\u2026,@,@:,>");
  check("the ? list's placeholder is the ? provider's", input().placeholder === "Type '?' to get help");
  type("?>");
  check("typing ?> enters the > mode", input().value === ">" && input().placeholder === `placeholder for ">"`);
  check("the > mode's rows replace the help list", optionLabels().join(",") === `row of ">"`);
  check("the jump never closed the panel", panel().hidden === false);
  key("Escape");
  jumpInput.dispose();
}

// --- Provider half: the placeholder provider --------------------------------------

{
  const placeholder = createPlaceholderProvider("Debugging is not available");
  const rows = placeholder.getItems("anything");
  check("a placeholder provider renders its single row", rows.length === 1 && rows[0]?.label === "Debugging is not available");
  const stateRegistry = createQuickAccessRegistry();
  stateRegistry.registerQuickAccessProvider({
    prefix: "",
    placeholder: "Search files by name",
    helpEntries: [],
    factory: () => ({ getItems: () => [{ label: "a.ts", accept: () => {} }] }),
  });
  const stateInput = new QuickInputService({ registry: stateRegistry });
  stateInput.quickAccess.show("");
  rows[0]?.accept();
  check(
    "a placeholder row's accept leaves quick input state unchanged",
    panel().hidden === false && optionLabels().join(",") === "a.ts",
  );
  key("Escape");
  stateInput.dispose();
}

// --- Provider half: includeHelp renders the modes list before recent files ---------

{
  const modesRegistry = createQuickAccessRegistry();
  modesRegistry.registerQuickAccessProvider({
    prefix: "",
    placeholder: "Search files by name",
    helpEntries: [{ description: "Go to File", commandId: "test.quickOpen", commandCenterOrder: 10 }],
    factory: () => ({
      getItems: () => [
        { label: "alpha.md", accept: () => {} },
        { label: "beta.md", accept: () => {} },
      ],
    }),
  });
  const { commands, menus, keybindings, context, history } = paletteSetup();
  commands.register("file.save", { title: "Save", category: "File", run: () => {} });
  menus.appendMenuItem(MenuId.CommandPalette, { command: "file.save" });
  const descriptors = createQuickAccessProviderDescriptors({
    commands,
    menus,
    keybindings,
    context,
    history,
    quickAccess: modesRegistry,
    show: () => {},
  });
  for (const descriptor of descriptors) {
    modesRegistry.registerQuickAccessProvider(descriptor);
  }

  // Cursor's own words for every placeholder and help row of the providers this file owns.
  const wording = Object.fromEntries(
    descriptors.map((descriptor) => [
      descriptor.prefix,
      [descriptor.placeholder, ...descriptor.helpEntries.map((entry) => entry.description)],
    ]),
  );
  const expectedWording = {
    ">": ["Type the name of a command to run.", "Show and Run Commands"],
    "%": ["Search for text in your workspace files.", "Search for Text"],
    "@": [
      "Type the name of a symbol to go to.",
      "Go to Symbol in Editor",
      "Go to Symbol in Editor by Category",
    ],
    "debug ": ["Type the name of a launch configuration to run.", "Start Debugging"],
    "task ": ["Type the name of a task to run.", "Run Task"],
    "?": ["Type '?' to get help on the actions you can take from here.", "Show all Quick Access Providers"],
  };
  check(
    "every descriptor this file owns uses Cursor's placeholder and help text",
    JSON.stringify(wording) === JSON.stringify(expectedWording),
  );
  const orders = Object.fromEntries(
    descriptors.flatMap((descriptor) =>
      descriptor.helpEntries
        .filter((entry) => entry.commandCenterOrder !== undefined)
        .map((entry) => [descriptor.prefix, [entry.commandCenterOrder, entry.commandCenterLabel ?? null, entry.commandId ?? null]]),
    ),
  );
  check(
    "the modes list's order, label, and command ids match Cursor's",
    JSON.stringify(orders) ===
      JSON.stringify({
        ">": [20, null, "workbench.action.showCommands"],
        "%": [25, null, "workbench.action.quickTextSearch"],
        "@": [40, null, "workbench.action.gotoSymbol"],
        "debug ": [50, null, "workbench.action.debug.selectandstart"],
        "task ": [60, null, null],
        "?": [70, "More", null],
      }),
  );

  const modesInput = new QuickInputService({ registry: modesRegistry, keybindings });
  modesInput.quickAccess.show("", { includeHelp: true });
  check(
    "includeHelp renders the modes list before the recent files",
    optionLabels().join(",") ===
      "Go to File,Show and Run Commands,Search for Text,Go to Symbol in Editor,Start Debugging,Run Task,More,alpha.md,beta.md",
  );
  check(
    "the unordered help entries stay out of the modes list",
    !optionLabels().includes("Go to Symbol in Editor by Category") && !optionLabels().includes("Show all Quick Access Providers"),
  );
  key("Escape");
  modesInput.quickAccess.show(">");
  check("the palette descriptor routes the > prefix", optionLabels().join(",") === "File: Save");
  key("Escape");
  modesInput.quickAccess.show("debug ");
  check("the debug descriptor renders its not-available row", optionLabels().join(",") === "Debugging is not available");
  key("Escape");
  modesInput.dispose();
  context.dispose();
}

if (failures.length > 0) {
  console.error(`quick-input: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("quick-input: all assertions passed");
