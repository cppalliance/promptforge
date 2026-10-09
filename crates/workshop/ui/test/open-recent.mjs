// Unit test for the Open Recent providers
// (src/parts/workspace/open-recent.ts): the menu provider's dynamic rows
// and the "" quick-access provider's accept, driven over injected stores
// and an injected command registry the way test/menus.mjs drives the
// menu half. Covers the workspace-file rows (TWF-003): a .pfwork entry
// in the recent-files store renders in 1_workspaces dispatching
// workbench.action.openWorkspace with the path argument, any case of the
// extension; a text entry still renders vscode.open in 3_files; the
// quick-access provider's accept dispatches the workspace command for a
// .pfwork hit and vscode.open for a text hit, a filter's match in the file
// name is highlighted, and a rejected run is logged naming the command that
// ran (the registry reports it as the toast, so no status message posts).
// Bundles the module with esbuild and drives it DOM-free.
// Run: node --test test/open-recent.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createRecentMenuProvider, createFileQuickAccessProvider, isWorkspaceFilePath } from "./src/parts/workspace/open-recent.ts";
      export { registerService } from "@workshop/platform/service-registry";
      export { STATUS_BAR } from "@workshop/platform/status-bar";
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
  // The status-bar token's module pulls colocated CSS; the test drives only the JS.
  loader: { ".css": "empty" },
});
const { createRecentMenuProvider, createFileQuickAccessProvider, isWorkspaceFilePath, registerService, STATUS_BAR } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets an accept's rejected command run reach its failure report.
async function flush() {
  for (let i = 0; i < 4; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const WORKSPACE = "C:\\work\\Alpha.pfwork";
const UPPER = "C:\\work\\Beta.PFWORK";
const NOTE = "C:\\work\\notes.md";

const statusMessages = [];
registerService(STATUS_BAR, () => ({
  showLocal: (label, severity) => statusMessages.push({ label, severity }),
}));

// --- The extension predicate ------------------------------------------------------

check("a .pfwork path is a workspace file", isWorkspaceFilePath(WORKSPACE));
check("the extension check is case-insensitive", isWorkspaceFilePath(UPPER));
check("a .md path is not a workspace file", !isWorkspaceFilePath(NOTE));
check("the extension must end the path", !isWorkspaceFilePath("C:\\work\\Alpha.pfwork.bak"));

// --- The menu provider -------------------------------------------------------------

{
  const treeState = { listing: () => undefined, cachedListings: () => [] };
  const recentFiles = { list: [WORKSPACE, NOTE, UPPER] };
  const rows = createRecentMenuProvider({ treeState, recentFiles })();

  const workspaceRow = rows.find((row) => row.args?.[0] === WORKSPACE);
  check(
    "a .pfwork entry renders in 1_workspaces dispatching workbench.action.openWorkspace with the path",
    workspaceRow?.command === "workbench.action.openWorkspace" &&
      workspaceRow?.group === "1_workspaces" &&
      workspaceRow?.title === "Alpha.pfwork" &&
      workspaceRow?.order === 0,
  );
  const upperRow = rows.find((row) => row.args?.[0] === UPPER);
  check(
    "an upper-case .PFWORK entry is a workspace row too",
    upperRow?.command === "workbench.action.openWorkspace" && upperRow?.group === "1_workspaces" && upperRow?.order === 1,
  );
  const noteRow = rows.find((row) => row.args?.[0] === NOTE);
  check(
    "a .md entry still renders vscode.open in 3_files",
    noteRow?.command === "vscode.open" && noteRow?.group === "3_files" && noteRow?.title === "notes.md" && noteRow?.order === 0,
  );
  check("workspace rows stay out of 3_files", !rows.some((row) => row.group === "3_files" && isWorkspaceFilePath(row.args[0])));
  check("the provider renders every recent entry once", rows.length === 3);
}

// --- The quick-access provider's accept ----------------------------------------------

{
  const executed = [];
  const commands = {
    execute: (id, ...args) => {
      executed.push({ id, args });
      return Promise.resolve(true);
    },
  };
  const treeState = { listing: () => undefined, cachedListings: () => [] };
  const recentFiles = { list: [WORKSPACE, NOTE] };
  const provider = createFileQuickAccessProvider({ treeState, recentFiles, commands });
  const items = provider.getItems("");
  check("the quick-access provider lists both entries", items.length === 2);

  items.find((item) => item.description === WORKSPACE)?.accept();
  check(
    "accepting a .pfwork hit dispatches workbench.action.openWorkspace with the path",
    executed.at(-1)?.id === "workbench.action.openWorkspace" && executed.at(-1)?.args?.[0] === WORKSPACE,
  );
  items.find((item) => item.description === NOTE)?.accept();
  check(
    "accepting a text hit dispatches vscode.open with the path",
    executed.at(-1)?.id === "vscode.open" && executed.at(-1)?.args?.[0] === NOTE,
  );
  check("each accept runs exactly one command", executed.length === 2);
}

// --- The matched text of a file name is highlighted --------------------------------------

{
  const treeState = { listing: () => undefined, cachedListings: () => [] };
  const recentFiles = { list: [WORKSPACE, NOTE] };
  const provider = createFileQuickAccessProvider({ treeState, recentFiles, commands: { execute: () => Promise.resolve(true) } });
  const named = provider.getItems("NOTE").find((item) => item.description === NOTE);
  check(
    "a filter that matches the file name highlights the match",
    named?.labelHighlights?.length === 1 && named.labelHighlights[0].start === 0 && named.labelHighlights[0].end === 4,
  );
  const byPath = provider.getItems("c:\\work\\n").find((item) => item.description === NOTE);
  check("a filter that matches only the path highlights nothing in the name", byPath !== undefined && byPath.labelHighlights === undefined);
  check("an empty filter highlights nothing", provider.getItems("").every((item) => item.labelHighlights === undefined));
}

// --- A rejected run: the registry reports it, the provider keeps the detail --------------

{
  const commands = {
    execute: () => Promise.reject(new Error("refused")),
  };
  const treeState = { listing: () => undefined, cachedListings: () => [] };
  const recentFiles = { list: [WORKSPACE, NOTE] };
  const items = createFileQuickAccessProvider({ treeState, recentFiles, commands }).getItems("");

  const logged = [];
  const realConsoleError = console.error;
  console.error = (...args) => logged.push(args);
  items.find((item) => item.description === WORKSPACE)?.accept();
  await flush();
  check(
    "a rejected workspace open is logged naming workbench.action.openWorkspace with its error",
    String(logged.at(-1)?.[0]).includes("workbench.action.openWorkspace") && logged.at(-1)?.[1]?.message === "refused",
  );
  items.find((item) => item.description === NOTE)?.accept();
  await flush();
  console.error = realConsoleError;
  check("a rejected text open is logged naming vscode.open", String(logged.at(-1)?.[0]).includes("'vscode.open'"));
  check("the provider posts no status message of its own for a failure", statusMessages.length === 0);
}

if (failures.length > 0) {
  console.error(`open-recent: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("open-recent: all assertions passed");
process.exit(0);
