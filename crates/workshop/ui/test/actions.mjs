// Unit test for the action registry (src/services/action-registry.ts):
// registerAction fans one descriptor out into the command, menu, and
// keybinding registries - the command with its metadata, one menu row
// per menu entry plus a CommandPalette row when f1 is set, and the
// keybinding rule with the precondition ANDed into its when. The
// returned DisposableStore unwinds every registration together, and
// malformed when/precondition/toggled/keybinding strings come back as
// ParseError values at registration with nothing registered. Bundles
// the modules with esbuild and drives them.
// Run: node --test test/actions.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createActionRegistry, registerAction } from "./src/services/action-registry.ts";
      export { CommandRegistry, Commands } from "./src/services/command-registry.ts";
      export { MenuRegistry, MenuId } from "./src/services/menu-registry.ts";
      export { createKeybindingsRegistry } from "./src/services/keybinding-registry.ts";
      export { ContextKeyService } from "./src/services/context-key-service.ts";
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
});
const {
  createActionRegistry,
  registerAction,
  CommandRegistry,
  Commands,
  MenuRegistry,
  MenuId,
  createKeybindingsRegistry,
  ContextKeyService,
} = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function chord(ctrl, shift, alt, meta, key) {
  return { ctrl, shift, alt, meta, key };
}

function setup() {
  const commands = new CommandRegistry();
  const menus = new MenuRegistry();
  const keybindings = createKeybindingsRegistry("windows");
  const actions = createActionRegistry({ commands, menus, keybindings, platform: "windows" });
  return { commands, menus, keybindings, actions };
}

{
  // One descriptor fans out into all three registries.
  const { commands, menus, keybindings, actions } = setup();
  let ran = 0;
  const result = actions.registerAction({
    id: "file.save",
    title: "Save",
    category: "File",
    f1: true,
    precondition: "editorTextFocus",
    toggled: "editorDirty",
    keybinding: { keybinding: "ctrl+s", weight: 100 },
    menu: [
      { id: MenuId.MenubarFileMenu, group: "1_save", order: 1, args: ["quiet"] },
      { id: "menubar/file/share", when: "hasProfile" },
    ],
    run: () => {
      ran += 1;
    },
  });
  check("a valid action registers ok", result.ok === true);
  const command = commands.lookup("file.save");
  check("the command is registered", command !== undefined);
  check("the command records its metadata", command?.title === "Save" && command?.category === "File" && command?.precondition === "editorTextFocus" && command?.toggled === "editorDirty");
  const fileRows = menus.getMenuItems(MenuId.MenubarFileMenu);
  check("the menu entry lands in its menu", fileRows.length === 1 && fileRows[0]?.command === "file.save" && fileRows[0]?.group === "1_save");
  check("the menu entry records its args", fileRows[0]?.args?.[0] === "quiet");
  const shareRows = menus.getMenuItems("menubar/file/share");
  check("a second menu entry lands in its own menu", shareRows.length === 1 && shareRows[0]?.when === "hasProfile");
  const paletteRows = menus.getMenuItems(MenuId.CommandPalette);
  check("f1 places a palette row", paletteRows.length === 1 && paletteRows[0]?.command === "file.save");
  const context = new ContextKeyService();
  context.createKey("editorTextFocus", true);
  const outcome = keybindings.getResolver().resolve(context, [chord(true, false, false, false, "s")]);
  check("the keybinding rule resolves", outcome.kind === "KbFound" && outcome.commandId === "file.save");
  check("the keybinding label is available", keybindings.lookupKeybinding("file.save")?.getLabel() === "Ctrl+S");
  await commands.execute("file.save");
  check("the command runs through the registry", ran === 1);
  context.dispose();
}

{
  // Without f1 there is no palette row.
  const { menus, actions } = setup();
  const result = actions.registerAction({ id: "view.zoom", title: "Zoom", run: () => {} });
  check("a minimal action registers ok", result.ok === true);
  check("no f1 means no palette row", menus.getMenuItems(MenuId.CommandPalette).length === 0);
  result.value.dispose();
}

{
  // The precondition is ANDed into the keybinding's when.
  const { keybindings, actions } = setup();
  const result = actions.registerAction({
    id: "editor.format",
    title: "Format",
    precondition: "editorTextFocus",
    keybinding: { keybinding: "ctrl+k ctrl+f", when: "editorLangId == 'rust'" },
    run: () => {},
  });
  check("an action with both precondition and when registers ok", result.ok === true);
  const context = new ContextKeyService();
  const focus = context.createKey("editorTextFocus", false);
  const lang = context.createKey("editorLangId", "plaintext");
  const chords = [chord(true, false, false, false, "k"), chord(true, false, false, false, "f")];
  check("the chord needs the precondition", keybindings.getResolver().resolve(context, chords).kind === "NoMatchingKb");
  focus.set(true);
  check("the chord needs the keybinding when", keybindings.getResolver().resolve(context, chords).kind === "NoMatchingKb");
  lang.set("rust");
  const outcome = keybindings.getResolver().resolve(context, chords);
  check("precondition AND when together release the chord", outcome.kind === "KbFound" && outcome.commandId === "editor.format");
  context.dispose();
  result.value.dispose();
}

{
  // A precondition alone becomes the keybinding's when.
  const { keybindings, actions } = setup();
  actions.registerAction({
    id: "editor.comment",
    title: "Comment",
    precondition: "editorTextFocus",
    keybinding: { keybinding: "ctrl+/" },
    run: () => {},
  });
  const context = new ContextKeyService();
  const focus = context.createKey("editorTextFocus", false);
  check("the precondition gates the chord", keybindings.getResolver().resolve(context, [chord(true, false, false, false, "/")]).kind === "NoMatchingKb");
  focus.set(true);
  check("a met precondition releases the chord", keybindings.getResolver().resolve(context, [chord(true, false, false, false, "/")]).kind === "KbFound");
  context.dispose();
}

{
  // The returned store unwinds every registration together.
  const { commands, menus, keybindings, actions } = setup();
  const result = actions.registerAction({
    id: "file.reopen",
    title: "Reopen",
    f1: true,
    keybinding: { keybinding: "ctrl+r" },
    menu: [{ id: MenuId.MenubarFileMenu }],
    run: () => {},
  });
  check("dispose-ready action registered ok", result.ok === true);
  result.value.dispose();
  check("dispose unregisters the command", commands.lookup("file.reopen") === undefined);
  check("dispose removes the menu row", menus.getMenuItems(MenuId.MenubarFileMenu).length === 0);
  check("dispose removes the palette row", menus.getMenuItems(MenuId.CommandPalette).length === 0);
  check("dispose removes the keybinding label", keybindings.lookupKeybinding("file.reopen") === undefined);
  const context = new ContextKeyService();
  check("dispose removes the keybinding rule", keybindings.getResolver().resolve(context, [chord(true, false, false, false, "r")]).kind === "NoMatchingKb");
  context.dispose();
}

{
  // Re-registering an id upserts instead of duplicating rows.
  const { commands, menus, actions } = setup();
  const first = actions.registerAction({ id: "a.b", title: "One", f1: true, run: () => {} });
  const second = actions.registerAction({ id: "a.b", title: "Two", f1: true, run: () => {} });
  check("re-registration is ok", first.ok === true && second.ok === true);
  check("re-registration upserts the command", commands.lookup("a.b")?.title === "Two");
  check("re-registration does not duplicate the palette row", menus.getMenuItems(MenuId.CommandPalette).length === 1);
  first.value.dispose();
  check("disposing the stale registration keeps the replacement", commands.lookup("a.b")?.title === "Two");
  second.value.dispose();
}

{
  // Malformed strings come back as errors and register nothing.
  const { commands, menus, keybindings, actions } = setup();
  const badPrecondition = actions.registerAction({
    id: "bad.precondition",
    title: "Bad",
    precondition: "a &&",
    keybinding: { keybinding: "ctrl+p" },
    menu: [{ id: MenuId.MenubarFileMenu }],
    run: () => {},
  });
  check("a malformed precondition is returned", badPrecondition.ok === false && typeof badPrecondition.error.message === "string");
  check("a malformed precondition registers no command", commands.lookup("bad.precondition") === undefined);
  check("a malformed precondition registers no menu row", menus.getMenuItems(MenuId.MenubarFileMenu).length === 0);
  check("a malformed precondition registers no keybinding", keybindings.lookupKeybinding("bad.precondition") === undefined);

  const badToggled = actions.registerAction({ id: "bad.toggled", title: "Bad", toggled: "x ==", run: () => {} });
  check("a malformed toggled is returned", badToggled.ok === false);
  check("a malformed toggled registers nothing", commands.lookup("bad.toggled") === undefined);

  const badMenuWhen = actions.registerAction({
    id: "bad.menuwhen",
    title: "Bad",
    menu: [{ id: MenuId.MenubarFileMenu, when: "(open" }],
    run: () => {},
  });
  check("a malformed menu when is returned", badMenuWhen.ok === false);
  check("a malformed menu when registers nothing", commands.lookup("bad.menuwhen") === undefined);

  const badKeyWhen = actions.registerAction({
    id: "bad.keywhen",
    title: "Bad",
    keybinding: { keybinding: "ctrl+q", when: "!!" },
    run: () => {},
  });
  check("a malformed keybinding when is returned", badKeyWhen.ok === false);
  check("a malformed keybinding when registers nothing", commands.lookup("bad.keywhen") === undefined);

  const badChord = actions.registerAction({
    id: "bad.chord",
    title: "Bad",
    keybinding: { keybinding: "ctrl+banana" },
    run: () => {},
  });
  check("a malformed chord is returned", badChord.ok === false);
  check("a malformed chord registers nothing", commands.lookup("bad.chord") === undefined);
  check("a malformed chord reaches no resolver", keybindings.getResolver().resolve(new ContextKeyService(), [chord(true, false, false, false, "p")]).kind === "NoMatchingKb");
}

{
  // The module-level registerAction writes to the shared registries.
  const result = registerAction({ id: "test.shared.action", title: "Shared", f1: true, run: () => {} });
  check("the shared registerAction is ok", result.ok === true);
  check("the shared registerAction reaches the shared commands", Commands.lookup("test.shared.action")?.title === "Shared");
  result.value.dispose();
  check("the shared registration unwinds", Commands.lookup("test.shared.action") === undefined);
}

if (failures.length > 0) {
  console.error(`actions: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("actions: all assertions passed");
process.exit(0);
