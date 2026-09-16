// Unit test for the command and menu registries in their services/ home
// (src/services/command-registry.ts, src/services/menu-registry.ts): the
// CommandAction shape (run plus title/category/precondition/toggled
// metadata, no label/shortcut/enabled), execute dispatching arguments and
// awaiting async runs, upsert-by-id with self-only disposal; the MenuId
// const object, MenuItem and SubmenuItem rows, appendMenuItem upsert,
// setProvider dynamic rows merged with static rows, and getMenuItems
// sorting (navigation group first, then groups lexically, then order,
// then title) so group boundaries - never a separator item kind - fall
// between contiguous group clusters. Also covers the workspace feature's
// Open Recent provider (plan step 17): dynamic root and recent-file rows
// merged with the static Reopen/More/Clear rows in sort order, and no
// rows when roots and history are both empty. Bundles the modules with
// esbuild and drives them DOM-free.
// Run: node --test test/menus.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { CommandRegistry, Commands, registerCommand, executeCommand } from "./src/services/command-registry.ts";
      export { MenuRegistry, Menus, MenuId, appendMenuItem } from "./src/services/menu-registry.ts";
      export { createRecentMenuProvider } from "./src/ui/workspace/open-recent.ts";
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
  // The provider's accept path imports the status-bar token, whose module
  // pulls colocated CSS; the test drives only the JS.
  loader: { ".css": "empty" },
});
const { CommandRegistry, MenuRegistry, MenuId, createRecentMenuProvider } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Command registry -----------------------------------------------------------

{
  const commands = new CommandRegistry();
  let runs = 0;
  let seenArgs = null;
  const registration = commands.register("test.run", {
    title: "Run",
    category: "Test",
    precondition: "isReady",
    toggled: "isToggled",
    run: (...args) => {
      runs += 1;
      seenArgs = args;
    },
  });
  const found = commands.lookup("test.run");
  check(
    "a registered action round-trips with its metadata",
    found?.title === "Run" && found?.category === "Test" && found?.precondition === "isReady" && found?.toggled === "isToggled",
  );
  check("an unknown command is not found", commands.lookup("test.nope") === undefined);

  const executed = await commands.execute("test.run", "a", 2);
  check("execute dispatches arguments and reports the run", executed === true && runs === 1 && seenArgs?.[0] === "a" && seenArgs?.[1] === 2);
  check("execute reports an unknown command as not run", (await commands.execute("test.nope")) === false);

  let resolved = false;
  commands.register("test.async", {
    run: () =>
      new Promise((resolve) => {
        setTimeout(() => {
          resolved = true;
          resolve();
        }, 0);
      }),
  });
  await commands.execute("test.async");
  check("execute awaits an async run", resolved === true);

  commands.register("test.run", { title: "Run Again", run: () => (runs += 10) });
  await commands.execute("test.run");
  check("re-registering upserts the action", runs === 11 && commands.lookup("test.run").title === "Run Again");
  registration.dispose();
  await commands.execute("test.run");
  check("disposing the stale registration leaves the upsert in place", runs === 21);
  const current = commands.lookup("test.run");
  check("the upsert carries no stale metadata", current.category === undefined && current.precondition === undefined);

  const disposable = commands.register("test.gone", { run: () => {} });
  disposable.dispose();
  check("disposing the current registration unregisters it", commands.lookup("test.gone") === undefined && (await commands.execute("test.gone")) === false);
}

// --- Menu registry: ids and rows --------------------------------------------------

{
  check(
    "the MenuId const object carries the well-known ids",
    MenuId.MenubarMainMenu === "menubar" && MenuId.MenubarFileMenu === "menubar/file" && MenuId.CommandPalette === "commandPalette",
  );

  const menus = new MenuRegistry();
  menus.appendMenuItem(MenuId.MenubarFileMenu, { command: "file.save", title: "Save", group: "1_modification", order: 1 });
  menus.appendMenuItem(MenuId.MenubarFileMenu, { command: "file.open", title: "Open", group: "navigation", order: 2 });
  menus.appendMenuItem(MenuId.MenubarFileMenu, { command: "file.new", title: "New", group: "navigation", order: 1 });
  menus.appendMenuItem(MenuId.MenubarFileMenu, { command: "file.untitled", title: "Untitled" });
  const rows = menus.getMenuItems(MenuId.MenubarFileMenu);
  check(
    "rows sort navigation first, then groups lexically, then order",
    rows.map((row) => row.command).join(",") === "file.untitled,file.new,file.open,file.save",
  );
  check("an ungrouped row joins the navigation group", rows[0].command === "file.untitled");
  check(
    "group clusters are contiguous so separators fall on boundaries",
    rows.slice(0, 3).every((row) => (row.group ?? "navigation") === "navigation") && rows[3].group === "1_modification",
  );

  menus.appendMenuItem(MenuId.MenubarFileMenu, { command: "file.open", title: "Open…", group: "navigation", order: 0 });
  const afterUpsert = menus.getMenuItems(MenuId.MenubarFileMenu);
  check(
    "an item upsert replaces the row without duplicating it",
    afterUpsert.length === 4 && afterUpsert[0].command === "file.open" && afterUpsert[0].title === "Open…",
  );

  const titles = [
    { command: "zeta", title: "Beta", group: "g", order: 1 },
    { command: "alpha", title: "Alpha", group: "g", order: 1 },
  ];
  for (const item of titles) menus.appendMenuItem("menu/titles", item);
  check(
    "equal group and order fall back to title",
    menus.getMenuItems("menu/titles").map((row) => row.title).join(",") === "Alpha,Beta",
  );
}

// --- Menu registry: submenus, providers, disposal -----------------------------------

{
  const menus = new MenuRegistry();
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: MenuId.MenubarFileMenu, title: "File", order: 1 });
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/edit", title: "Edit", order: 2 });
  const top = menus.getMenuItems(MenuId.MenubarMainMenu);
  check(
    "a top-level menu is a submenu row on the main menu",
    top.length === 2 && top[0].submenu === "menubar/file" && top[0].title === "File" && top[1].submenu === "menubar/edit",
  );

  menus.appendMenuItem("menu/recent", { command: "reopen", title: "Reopen Closed Editor", group: "1_reopen", order: 1 });
  menus.appendMenuItem("menu/recent", { command: "more", title: "More…", group: "3_more", order: 1 });
  let recent = ["a.ts", "b.ts"];
  const providerDisposable = menus.setProvider("menu/recent", () =>
    recent.map((name, index) => ({ command: `open:${name}`, title: name, group: "2_history", order: index })),
  );
  const merged = menus.getMenuItems("menu/recent");
  check(
    "provider rows merge with static rows in sort order",
    merged.map((row) => row.title).join(",") === "Reopen Closed Editor,a.ts,b.ts,More…",
  );
  recent = ["c.ts"];
  check(
    "provider rows are re-read at every call",
    menus.getMenuItems("menu/recent").map((row) => row.title).join(",") === "Reopen Closed Editor,c.ts,More…",
  );
  providerDisposable.dispose();
  check(
    "disposing the provider drops its rows",
    menus.getMenuItems("menu/recent").map((row) => row.title).join(",") === "Reopen Closed Editor,More…",
  );

  const stale = menus.appendMenuItem("menu/disp", { command: "one", title: "One" });
  menus.appendMenuItem("menu/disp", { command: "one", title: "One Again" });
  stale.dispose();
  check(
    "disposing a stale row leaves its upsert in place",
    menus.getMenuItems("menu/disp").map((row) => row.title).join(",") === "One Again",
  );
  const live = menus.appendMenuItem("menu/disp", { command: "two", title: "Two" });
  live.dispose();
  check(
    "disposing a live row removes only it",
    menus.getMenuItems("menu/disp").map((row) => row.title).join(",") === "One Again",
  );
}

// --- Open Recent: the workspace provider's dynamic rows (step 17) -------------

{
  const menus = new MenuRegistry();
  menus.appendMenuItem(MenuId.MenubarRecentMenu, {
    command: "workbench.action.reopenClosedEditor",
    title: "Reopen Closed Editor",
    group: "1_editor",
  });
  menus.appendMenuItem(MenuId.MenubarRecentMenu, {
    command: "workbench.action.openRecent",
    title: "More...",
    group: "y_more",
  });
  menus.appendMenuItem(MenuId.MenubarRecentMenu, {
    command: "workbench.action.clearRecentFiles",
    title: "Clear Recently Opened...",
    group: "z_clear",
  });

  const rootsListing = {
    path: null,
    entries: [
      { name: "alpha", path: "C:\\alpha", kind: "directory", size: 0, modifiedMs: 1, exists: true },
      { name: "beta", path: "C:\\beta", kind: "directory", size: 0, modifiedMs: 2, exists: true },
    ],
  };
  const treeState = { listing: (path) => (path === "" ? rootsListing : undefined) };
  const recentFiles = { list: ["C:\\alpha\\one.ts", "C:\\beta\\two.ts"] };
  menus.setProvider(MenuId.MenubarRecentMenu, createRecentMenuProvider({ treeState, recentFiles }));

  const rows = menus.getMenuItems(MenuId.MenubarRecentMenu);
  check(
    "Open Recent merges roots and recent files with the static rows in sort order",
    rows.map((row) => row.title).join(",") ===
      "Reopen Closed Editor,alpha,beta,one.ts,two.ts,More...,Clear Recently Opened...",
  );
  const rootRow = rows.find((row) => row.title === "alpha");
  check(
    "a root row dispatches vscode.openFolder with the root path",
    rootRow?.command === "vscode.openFolder" && rootRow?.args?.[0] === "C:\\alpha" && rootRow?.group === "2_roots",
  );
  const fileRow = rows.find((row) => row.title === "one.ts");
  check(
    "a recent-file row dispatches vscode.open with the file path",
    fileRow?.command === "vscode.open" && fileRow?.args?.[0] === "C:\\alpha\\one.ts" && fileRow?.group === "3_files",
  );

  const empty = new MenuRegistry();
  empty.setProvider(
    MenuId.MenubarRecentMenu,
    createRecentMenuProvider({ treeState: { listing: () => undefined }, recentFiles: { list: [] } }),
  );
  check(
    "the provider answers no rows when roots and history are both empty",
    empty.getMenuItems(MenuId.MenubarRecentMenu).length === 0,
  );
}

if (failures.length > 0) {
  console.error(`menus: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("menus: all assertions passed");
