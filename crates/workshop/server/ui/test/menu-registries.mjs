// Unit test for the workbench menu registries
// (src/services/command-registry.ts, src/services/menu-registry.ts) and
// the menubar's button generation (src/ui/menu/menubar.ts). Commands are
// actions keyed by id in the command registry; menu rows are command
// references or submenu pointers per menu id in the menu registry; the
// menubar generates the title bar's buttons from the root menu's
// submenu rows. Bundles the modules with esbuild and drives them
// against jsdom built from the real index.html. Covers: command
// registration, lookup, execution with arguments, upsert, and disposal;
// menu row sort order, upsert by id, submenu rows, provider rows merged
// with static rows and re-read at every call; button generation with
// data-menu last-segment selectors, registry sort order, command rows
// on the root menu not becoming buttons, and disposal.
// Run: node --test test/menu-registries.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.HTMLButtonElement = window.HTMLButtonElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { CommandRegistry } from "./src/services/command-registry.ts";
      export { MenuRegistry, MenuId } from "./src/services/menu-registry.ts";
      export { ContextKeyService } from "./src/services/context-key-service.ts";
      export { createKeybindingsRegistry } from "./src/services/keybinding-registry.ts";
      export { Menubar, appendMenubarButtons } from "./src/ui/menu/menubar.ts";
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
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});
const { CommandRegistry, MenuRegistry, MenuId, ContextKeyService, createKeybindingsRegistry, Menubar, appendMenubarButtons } =
  await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Command registry ---------------------------------------------------------

{
  const commands = new CommandRegistry();
  const runs = [];
  const registration = commands.register("test.run", { title: "Run", run: (...args) => runs.push(args) });
  check("a registered command is found by lookup", commands.lookup("test.run")?.title === "Run");
  check("an unknown command is not found", commands.lookup("test.nope") === undefined);
  check(
    "execute runs the command with its arguments and reports it",
    (await commands.execute("test.run", 1, "x")) === true && runs.length === 1 && runs[0].join(",") === "1,x",
  );
  check("execute reports an unknown command as not run", (await commands.execute("test.nope")) === false);

  let asyncDone = false;
  commands.register("test.async", {
    run: async () => {
      await Promise.resolve();
      asyncDone = true;
    },
  });
  await commands.execute("test.async");
  check("execute awaits an async run", asyncDone === true);

  commands.register("test.run", { title: "Run Again", run: () => runs.push(["again"]) });
  await commands.execute("test.run");
  check(
    "re-registering upserts the action",
    runs.length === 2 && runs[1][0] === "again" && commands.lookup("test.run").title === "Run Again",
  );
  registration.dispose();
  await commands.execute("test.run");
  check("disposing the original registration leaves the upsert in place", runs.length === 3);
}

// --- Menu registry --------------------------------------------------------------

{
  const menus = new MenuRegistry();
  menus.appendMenuItem("menubar/file", { command: "b.two", group: "1_b", order: 2 });
  menus.appendMenuItem("menubar/file", { command: "a.one" });
  menus.appendMenuItem("menubar/file", { command: "b.one", group: "1_b", order: 1 });
  menus.appendMenuItem("menubar/file", { command: "c.one", group: "2_c" });
  const ids = (rows) => rows.map((row) => ("submenu" in row ? row.submenu : row.command)).join(",");
  check(
    "rows sort navigation first, then groups lexically, then order",
    ids(menus.getMenuItems("menubar/file")) === "a.one,b.one,b.two,c.one",
  );

  menus.appendMenuItem("menubar/file", { command: "a.one", title: "Renamed" });
  const rows = menus.getMenuItems("menubar/file");
  check(
    "an upsert by command id keeps one row with the new data",
    rows.length === 4 && rows[0].title === "Renamed",
  );

  menus.appendMenuItem("menubar/file", { submenu: "menubar/file/recent", title: "Recent", group: "3_z" });
  const last = menus.getMenuItems("menubar/file").at(-1);
  check(
    "a submenu row round-trips its menu id and title",
    last !== undefined && "submenu" in last && last.submenu === "menubar/file/recent" && last.title === "Recent",
  );

  let dynamic = [{ command: "p.one" }];
  const providerReg = menus.setProvider("menubar/file", () => dynamic);
  check(
    "provider rows merge with static rows in sort order",
    ids(menus.getMenuItems("menubar/file")) === "a.one,p.one,b.one,b.two,c.one,menubar/file/recent",
  );
  dynamic = [{ command: "p.two", group: "2_c" }];
  check(
    "provider rows are re-read at every call",
    ids(menus.getMenuItems("menubar/file")) === "a.one,b.one,b.two,c.one,p.two,menubar/file/recent",
  );
  providerReg.dispose();
  check(
    "disposing the provider drops its rows",
    ids(menus.getMenuItems("menubar/file")) === "a.one,b.one,b.two,c.one,menubar/file/recent",
  );

  const rowReg = menus.appendMenuItem("menubar/file", { command: "d.one" });
  menus.appendMenuItem("menubar/file", { command: "d.one", title: "Replacement" });
  rowReg.dispose();
  check(
    "disposing a stale row registration leaves its replacement in place",
    menus.getMenuItems("menubar/file").some((row) => !("submenu" in row) && row.command === "d.one" && row.title === "Replacement"),
  );
}

// --- Button generation -----------------------------------------------------------

{
  const nav = window.document.createElement("nav");
  const buttons = appendMenubarButtons(nav, [
    { submenu: "menubar/file", title: "File" },
    { submenu: "menubar/view/appearance", title: "Appearance" },
  ]);
  check("one button per submenu row", buttons.length === 2 && nav.children.length === 2);
  check(
    "data-menu carries the menu id's last segment",
    buttons[0].dataset.menu === "file" && buttons[1].dataset.menu === "appearance",
  );
  check(
    "buttons are type=button with the popup aria state",
    buttons.every(
      (button) =>
        button.type === "button" &&
        button.getAttribute("aria-haspopup") === "menu" &&
        button.getAttribute("aria-expanded") === "false",
    ),
  );
  check("the button label is the row title", buttons[0].textContent === "File");
}

// --- Menubar: buttons from the root menu in sort order ----------------------------

{
  const menus = new MenuRegistry();
  const commands = new CommandRegistry();
  commands.register("file.new", { title: "New File", run: () => {} });
  // Registered out of order on purpose; a command row on the root menu
  // is not a top-level menu and must not become a button.
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/view", title: "View", order: 3 });
  menus.appendMenuItem(MenuId.MenubarMainMenu, { command: "file.new" });
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/file", title: "File", order: 1 });
  menus.appendMenuItem(MenuId.MenubarMainMenu, { submenu: "menubar/edit", title: "Edit", order: 2 });

  const nav = window.document.querySelector(".ws-window-titlebar__menus");
  const menubar = new Menubar(nav, {
    menus,
    commands,
    contextKeys: new ContextKeyService(),
    keybindings: createKeybindingsRegistry("linux"),
  });
  const buttons = [...nav.querySelectorAll(".ws-window-titlebar__menu")];
  check(
    "the menubar generates the root menu's submenu rows in sort order",
    buttons.map((button) => button.textContent).join(",") === "File,Edit,View",
  );
  check(
    "the generated buttons carry the last-segment selectors",
    buttons.map((button) => button.dataset.menu).join(",") === "file,edit,view",
  );
  menubar.dispose();
  check("disposal removes the generated buttons", nav.querySelectorAll(".ws-window-titlebar__menu").length === 0);
}

if (failures.length > 0) {
  console.error(`menu-registries: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("menu-registries: all assertions passed");
