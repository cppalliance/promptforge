// Quick input speaks Cursor's words. After bundling the contribution modules
// that register the providers (quickinput owns >, %, @, debug, task, and ?;
// the workspace feature owns the default file provider; the editor feature
// owns :), the shared registry holds Cursor's exact placeholder and help text
// for every provider, the command-center order, label, and command id of each
// help entry, and the real widget renders both lists from them: the modes
// list the folder-name button opens (the ordered entries, with prefix
// descriptions and keybinding chips) and the ? list (a prefix label per row,
// the help text beside it, sorted by prefix, with ? itself left out).
// Run: node --test test/quick-access-wording.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/parts/menu/stubs.contribution.ts";
      import "./src/parts/editor/editor.contribution.ts";
      import "./src/parts/workspace/workspace.contribution.ts";
      import "./src/parts/quickinput/quickinput.contribution.ts";
      import "./src/parts/quickinput/commands-history.ts";
      export { QuickInputService } from "./src/parts/quickinput/quick-input.ts";
      export { QuickAccessRegistry } from "@workshop/platform/quick-access-registry";
      export { registerService } from "@workshop/platform/service-registry";
      export { QUICK_INPUT_SERVICE } from "./src/services/quick-input-service.ts";
      export { TREE_STATE } from "./src/services/tree-state-service.ts";
      export { RECENT_FILES_STORE } from "./src/services/recent-files-store.ts";
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
  alias: {
    "@tauri-apps/plugin-dialog": path.join(uiDir, "helpers", "tauri-dialog-stub.mjs"),
    "@tauri-apps/api/event": path.join(uiDir, "helpers", "tauri-event-stub.mjs"),
    "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
    "@tauri-apps/api/webviewWindow": path.join(uiDir, "helpers", "tauri-webview-stub.mjs"),
  },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLInputElement = window.HTMLInputElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundlePath = path.join(os.tmpdir(), "promptforge-quick-access-wording-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  QuickInputService,
  QuickAccessRegistry,
  registerService,
  QUICK_INPUT_SERVICE,
  TREE_STATE,
  RECENT_FILES_STORE,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- The registered descriptors -------------------------------------------------

const providers = QuickAccessRegistry.getQuickAccessProviders();
const wording = Object.fromEntries(
  providers.map((provider) => [
    provider.prefix,
    [provider.placeholder, ...provider.helpEntries.map((entry) => entry.description)],
  ]),
);
const expectedWording = {
  "": [
    "Search files, content, and symbols (append : to go to line or @ to go to symbol)",
    "Go to File",
  ],
  ":": [
    "Type the line number and optional column to go to (e.g. 42:5 for line 42 and column 5).",
    "Go to Line/Column",
  ],
  ">": ["Type the name of a command to run.", "Show and Run Commands"],
  "%": ["Search for text in your workspace files.", "Search for Text"],
  "@": ["Type the name of a symbol to go to.", "Go to Symbol in Editor", "Go to Symbol in Editor by Category"],
  "debug ": ["Type the name of a launch configuration to run.", "Start Debugging"],
  "task ": ["Type the name of a task to run.", "Run Task"],
  "?": ["Type '?' to get help on the actions you can take from here.", "Show all Quick Access Providers"],
};
for (const [prefix, expected] of Object.entries(expectedWording)) {
  check(
    `the "${prefix}" provider uses Cursor's placeholder and help text`,
    JSON.stringify(wording[prefix]) === JSON.stringify(expected),
  );
}
check(
  "no provider is registered beyond Cursor's list",
  Object.keys(wording).sort().join("|") === Object.keys(expectedWording).sort().join("|"),
);

const entryOf = (prefix, index = 0) => providers.find((provider) => provider.prefix === prefix)?.helpEntries[index];
check(
  "Go to File is first in the modes list and names quickOpen",
  entryOf("")?.commandCenterOrder === 10 && entryOf("")?.commandId === "workbench.action.quickOpen",
);
check(
  "Go to Line/Column names gotoLine and is not in the modes list",
  entryOf(":")?.commandId === "workbench.action.gotoLine" && entryOf(":")?.commandCenterOrder === undefined,
);
check("the ? entry is last, labelled More", entryOf("?")?.commandCenterOrder === 70 && entryOf("?")?.commandCenterLabel === "More");

// --- The real widget over the real registrations ----------------------------------

// The file provider reads two stores at call time; empty fakes keep its rows out of the way.
registerService(TREE_STATE, () => ({ listing: () => undefined, cachedListings: () => [] }));
registerService(RECENT_FILES_STORE, () => ({ list: [], clear() {}, record() {} }));
const widget = new QuickInputService();
registerService(QUICK_INPUT_SERVICE, () => widget);

const panel = () => window.document.querySelector(".ws-quick-input");
const input = () => panel().querySelector("input");
const options = () => [...panel().querySelectorAll('[role="option"]')];
const text = (row, selector) => row.querySelector(selector)?.textContent;
const chips = (row) => [...row.querySelectorAll(".ws-quick-input__key")].map((chip) => chip.textContent).join("+");

widget.quickAccess.show("", { includeHelp: true });
{
  const rows = options();
  check(
    "the modes list reads Cursor's seven rows in order",
    rows.map((row) => text(row, ".ws-quick-input__option-label")).join(",") ===
      "Go to File,Show and Run Commands,Search for Text,Go to Symbol in Editor,Start Debugging,Run Task,More",
  );
  check(
    "each mode row shows its prefix, the default mode none",
    rows.map((row) => text(row, ".ws-quick-input__option-description") ?? "").join("|") === "|>|%|@|debug |task |?",
  );
  check("Go to File shows Ctrl+P", chips(rows[0]) === "Ctrl+P");
  check("Show and Run Commands shows Ctrl+Shift+P", chips(rows[1]) === "Ctrl+Shift+P");
  check("Go to Symbol in Editor shows Ctrl+Shift+O", chips(rows[3]) === "Ctrl+Shift+O");
  check("a mode without a keybinding shows no chips", chips(rows[2]) === "" && chips(rows[5]) === "" && chips(rows[6]) === "");
  check("the placeholder is Cursor's file search text", input().placeholder === expectedWording[""][0]);
  rows[6].click();
  check("the More row enters the ? mode", input().value === "?" && input().placeholder === expectedWording["?"][0]);
}

{
  // The ? list: one row per help entry, sorted by prefix, ? itself left out.
  const rows = options();
  check(
    "the ? list labels each row with its prefix and the default mode with an ellipsis",
    rows.map((row) => text(row, ".ws-quick-input__option-label")).join(",") === "\u2026,:,@,@:,%,>,debug ,task ",
  );
  check(
    "the ? list describes each row with its help text",
    rows.map((row) => text(row, ".ws-quick-input__option-description")).join("|") ===
      "Go to File|Go to Line/Column|Go to Symbol in Editor|Go to Symbol in Editor by Category|Search for Text|Show and Run Commands|Start Debugging|Run Task",
  );
  check("the ? list shows Go to Line/Column's Ctrl+G", chips(rows[1]) === "Ctrl+G");
  rows[5].click();
  check("a ? row enters its mode", input().value === ">" && input().placeholder === expectedWording[">"][0]);
  // Typing ?% from the help list jumps straight to the % mode.
  input().value = "?";
  input().dispatchEvent(new window.Event("input", { bubbles: true }));
  input().value = "?%";
  input().dispatchEvent(new window.Event("input", { bubbles: true }));
  check("typing ?% enters the % mode", input().value === "%" && input().placeholder === expectedWording["%"][0]);
}

widget.dispose();

if (failures.length > 0) {
  console.error(`quick-access-wording: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("quick-access-wording: all assertions passed");
process.exit(0);
