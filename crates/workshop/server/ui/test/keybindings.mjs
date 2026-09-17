// Unit test for the keybinding parser, resolver, and registry
// (src/services/keybinding-parser.ts, src/services/keybinding-resolver.ts,
// src/services/keybinding-registry.ts): chord-string parsing with the
// ctrlcmd token resolved per platform, KeyboardEvent mapping through
// event.code (never event.key, so Ctrl+Shift+= reads as = and Numpad0 is
// distinct from Digit0), the pure resolver's NoMatchingKb /
// MoreChordsNeeded / KbFound outcomes with when filtering and weight
// tiers, and the registry's mac/linux overrides, disposable
// unregistration, and per-platform lookupKeybinding labels. Bundles the
// modules with esbuild and drives them.
// Run: node --test test/keybindings.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { parseKeybinding, chordFromKeyboardEvent, formatChord, formatKeybinding, detectPlatform } from "./src/services/keybinding-parser.ts";
      export { KeybindingResolver } from "./src/services/keybinding-resolver.ts";
      export { KeybindingsRegistry, KeybindingWeight, createKeybindingsRegistry } from "./src/services/keybinding-registry.ts";
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
  parseKeybinding,
  chordFromKeyboardEvent,
  formatChord,
  formatKeybinding,
  detectPlatform,
  KeybindingResolver,
  KeybindingsRegistry,
  KeybindingWeight,
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

function sameChord(a, b) {
  return a.ctrl === b.ctrl && a.shift === b.shift && a.alt === b.alt && a.meta === b.meta && a.key === b.key;
}

function parseOk(text, platform) {
  const result = parseKeybinding(text, platform);
  if (!result.ok) return `parse-error: ${result.error.message}`;
  return result.value;
}

function keyEvent(code, mods = {}) {
  return {
    code,
    ctrlKey: mods.ctrl === true,
    shiftKey: mods.shift === true,
    altKey: mods.alt === true,
    metaKey: mods.meta === true,
  };
}

// --- Platform detection -------------------------------------------------------

check("detectPlatform answers a known platform", ["windows", "mac", "linux"].includes(detectPlatform()));

// --- Chord parsing ------------------------------------------------------------

{
  const chords = parseOk("ctrl+s", "windows");
  check("ctrl+s parses to one chord", Array.isArray(chords) && chords.length === 1);
  check("ctrl+s sets ctrl and the key", Array.isArray(chords) && sameChord(chords[0], chord(true, false, false, false, "s")));
}
{
  const chords = parseOk("ctrl+m ctrl+o", "windows");
  check("a chord sequence parses to two chords", Array.isArray(chords) && chords.length === 2);
  check(
    "the second chord parses",
    Array.isArray(chords) && sameChord(chords[1], chord(true, false, false, false, "o")),
  );
}
check("modifier and key case is insignificant", sameChord(parseOk("Ctrl+Shift+P", "windows")[0], chord(true, true, false, false, "p")));
check("cmd is a meta alias", sameChord(parseOk("cmd+p", "mac")[0], chord(false, false, false, true, "p")));
check("meta parses", sameChord(parseOk("meta+p", "windows")[0], chord(false, false, false, true, "p")));
check("alt parses", sameChord(parseOk("alt+up", "windows")[0], chord(false, false, true, false, "up")));
check("several modifiers combine", sameChord(parseOk("ctrl+alt+shift+f", "linux")[0], chord(true, true, true, false, "f")));

// --- ctrlcmd platform resolution ------------------------------------------------

check("ctrlcmd is meta on mac", sameChord(parseOk("ctrlcmd+s", "mac")[0], chord(false, false, false, true, "s")));
check("ctrlcmd is ctrl on windows", sameChord(parseOk("ctrlcmd+s", "windows")[0], chord(true, false, false, false, "s")));
check("ctrlcmd is ctrl on linux", sameChord(parseOk("ctrlcmd+s", "linux")[0], chord(true, false, false, false, "s")));

// --- Key vocabulary ---------------------------------------------------------------

check("a digit key parses", sameChord(parseOk("ctrl+0", "windows")[0], chord(true, false, false, false, "0")));
check("a function key parses", sameChord(parseOk("f5", "windows")[0], chord(false, false, false, false, "f5")));
check("a numpad key parses", sameChord(parseOk("ctrl+numpad0", "windows")[0], chord(true, false, false, false, "numpad0")));
check("a named key parses", sameChord(parseOk("ctrl+pagedown", "windows")[0], chord(true, false, false, false, "pagedown")));
check("punctuation parses by glyph", sameChord(parseOk("ctrl+shift+=", "windows")[0], chord(true, true, false, false, "=")));
check("backquote parses", sameChord(parseOk("ctrl+`", "windows")[0], chord(true, false, false, false, "`")));
check("surrounding whitespace is insignificant", sameChord(parseOk("  ctrl+s  ", "windows")[0], chord(true, false, false, false, "s")));

// --- Parse errors as values --------------------------------------------------------

check("an empty string is a parse error", !parseKeybinding("", "windows").ok);
check("a dangling modifier is a parse error", !parseKeybinding("ctrl+", "windows").ok);
check("a bare modifier is a parse error", !parseKeybinding("ctrl", "windows").ok);
check("an unknown modifier is a parse error", !parseKeybinding("hyper+s", "windows").ok);
check("an unknown key is a parse error", !parseKeybinding("ctrl+banana", "windows").ok);
check("a trailing plus is a parse error", !parseKeybinding("ctrl+s+", "windows").ok);
check("a space inside a chord is a parse error", !parseKeybinding("ctrl + s", "windows").ok);
{
  const result = parseKeybinding("ctrl+banana", "windows");
  check("a parse error carries a message and offset", !result.ok && typeof result.error.message === "string" && typeof result.error.offset === "number");
}

// --- KeyboardEvent mapping through event.code ----------------------------------------

check("KeyS maps to s", sameChord(chordFromKeyboardEvent(keyEvent("KeyS", { ctrl: true })), chord(true, false, false, false, "s")));
check(
  "Equal with shift still reads as =",
  sameChord(chordFromKeyboardEvent(keyEvent("Equal", { ctrl: true, shift: true })), chord(true, true, false, false, "=")),
);
check("Numpad0 is distinct from Digit0", sameChord(chordFromKeyboardEvent(keyEvent("Numpad0", { ctrl: true })), chord(true, false, false, false, "numpad0")));
check("Digit0 maps to 0", sameChord(chordFromKeyboardEvent(keyEvent("Digit0", { ctrl: true })), chord(true, false, false, false, "0")));
check("ArrowDown maps to down", sameChord(chordFromKeyboardEvent(keyEvent("ArrowDown", { alt: true })), chord(false, false, true, false, "down")));
check("a modifier-only event maps to undefined", chordFromKeyboardEvent(keyEvent("ControlLeft", { ctrl: true })) === undefined);
check("an unmapped code maps to undefined", chordFromKeyboardEvent(keyEvent("AudioVolumeUp")) === undefined);

// --- Labels ---------------------------------------------------------------------------

check("a chord label renders Ctrl+M", formatChord(chord(true, false, false, false, "m"), "windows") === "Ctrl+M");
check(
  "a sequence label renders Ctrl+M Ctrl+O",
  formatKeybinding([chord(true, false, false, false, "m"), chord(true, false, false, false, "o")], "windows") === "Ctrl+M Ctrl+O",
);
check("meta renders Cmd on mac", formatChord(chord(false, false, false, true, "s"), "mac") === "Cmd+S");
check("meta does not render Cmd off mac", formatChord(chord(false, false, false, true, "s"), "windows") !== "Cmd+S");
check("numpad0 labels as NumPad0", formatChord(chord(true, false, false, false, "numpad0"), "windows") === "Ctrl+NumPad0");
check("pagedown labels as PageDown", formatChord(chord(true, false, false, false, "pagedown"), "windows") === "Ctrl+PageDown");
check("punctuation labels as its glyph", formatChord(chord(true, true, false, false, "="), "windows") === "Ctrl+Shift+=");

// --- The resolver ------------------------------------------------------------------------

function rule(commandId, keys, options = {}) {
  return {
    commandId,
    chords: keys,
    when: options.when,
    weight: options.weight ?? 0,
    order: options.order ?? 0,
  };
}

const ctrlM = [chord(true, false, false, false, "m")];
const ctrlMO = [chord(true, false, false, false, "m"), chord(true, false, false, false, "o")];

{
  const resolver = new KeybindingResolver([]);
  const outcome = resolver.resolve(new ContextKeyService(), ctrlM);
  check("no rules is NoMatchingKb", outcome.kind === "NoMatchingKb");
}
{
  const resolver = new KeybindingResolver([rule("editor.save", ctrlM)]);
  const outcome = resolver.resolve(new ContextKeyService(), ctrlM);
  check("an exact match is KbFound", outcome.kind === "KbFound" && outcome.commandId === "editor.save");
}
{
  const resolver = new KeybindingResolver([rule("workbench.open", ctrlMO)]);
  const first = resolver.resolve(new ContextKeyService(), ctrlM);
  check("a prefix of a longer rule is MoreChordsNeeded", first.kind === "MoreChordsNeeded");
  const second = resolver.resolve(new ContextKeyService(), ctrlMO);
  check("the full chord sequence is KbFound", second.kind === "KbFound" && second.commandId === "workbench.open");
}
{
  const resolver = new KeybindingResolver([rule("editor.save", ctrlM)]);
  const outcome = resolver.resolve(new ContextKeyService(), [chord(true, false, false, false, "x")]);
  check("an unrelated chord is NoMatchingKb", outcome.kind === "NoMatchingKb");
}

// when filtering against a live context service
{
  const context = new ContextKeyService();
  const focus = context.createKey("editorTextFocus", false);
  const resolver = new KeybindingResolver([rule("editor.save", ctrlM, { when: "editorTextFocus" })]);
  check("a failing when hides the rule", resolver.resolve(context, ctrlM).kind === "NoMatchingKb");
  focus.set(true);
  const outcome = resolver.resolve(context, ctrlM);
  check("a passing when finds the rule", outcome.kind === "KbFound" && outcome.commandId === "editor.save");
  context.dispose();
}
{
  // A failing when on the exact rule does not block a longer candidate.
  const context = new ContextKeyService();
  const resolver = new KeybindingResolver([
    rule("short.cmd", ctrlM, { when: "false" }),
    rule("long.cmd", ctrlMO),
  ]);
  check("a longer candidate survives a failed when", resolver.resolve(context, ctrlM).kind === "MoreChordsNeeded");
  context.dispose();
}

// weights and registration order
{
  const low = rule("low.cmd", ctrlM, { weight: KeybindingWeight.EditorContrib, order: 0 });
  const high = rule("high.cmd", ctrlM, { weight: KeybindingWeight.WorkbenchContrib, order: 1 });
  const resolver = new KeybindingResolver([low, high]);
  const outcome = resolver.resolve(new ContextKeyService(), ctrlM);
  check("the higher weight wins", outcome.kind === "KbFound" && outcome.commandId === "high.cmd");
}
{
  const first = rule("first.cmd", ctrlM, { weight: KeybindingWeight.WorkbenchContrib, order: 0 });
  const second = rule("second.cmd", ctrlM, { weight: KeybindingWeight.WorkbenchContrib, order: 1 });
  const resolver = new KeybindingResolver([first, second]);
  const outcome = resolver.resolve(new ContextKeyService(), ctrlM);
  check("last registered wins at equal weight", outcome.kind === "KbFound" && outcome.commandId === "second.cmd");
}

// hasRuleForChord
{
  const resolver = new KeybindingResolver([rule("gated.cmd", ctrlMO, { when: "false" })]);
  check("hasRuleForChord ignores when", resolver.hasRuleForChord(chord(true, false, false, false, "m")) === true);
  check("hasRuleForChord is false for an unbound chord", resolver.hasRuleForChord(chord(true, false, false, false, "x")) === false);
}

// --- The registry ---------------------------------------------------------------------------

check("KeybindingWeight carries the VS Code tiers", KeybindingWeight.EditorCore === 0 && KeybindingWeight.EditorContrib === 100 && KeybindingWeight.WorkbenchContrib === 200 && KeybindingWeight.BuiltinExtension === 300 && KeybindingWeight.ExternalExtension === 400);

{
  const registry = createKeybindingsRegistry("windows");
  const registration = registry.registerKeybindingRule({ id: "editor.save", keybinding: "ctrlcmd+s" });
  const resolver = registry.getResolver();
  const outcome = resolver.resolve(new ContextKeyService(), [chord(true, false, false, false, "s")]);
  check("a registered rule resolves", outcome.kind === "KbFound" && outcome.commandId === "editor.save");
  registration.dispose();
  const after = registry.getResolver().resolve(new ContextKeyService(), [chord(true, false, false, false, "s")]);
  check("disposing the registration unbinds the rule", after.kind === "NoMatchingKb");
}
{
  // The mac override replaces the default keybinding on mac only.
  const macRegistry = createKeybindingsRegistry("mac");
  macRegistry.registerKeybindingRule({ id: "editor.save", keybinding: "ctrl+s", mac: "cmd+shift+s" });
  const macOutcome = macRegistry.getResolver().resolve(new ContextKeyService(), [chord(false, true, false, true, "s")]);
  check("the mac override binds on mac", macOutcome.kind === "KbFound" && macOutcome.commandId === "editor.save");
  const macDefault = macRegistry.getResolver().resolve(new ContextKeyService(), [chord(true, false, false, false, "s")]);
  check("the default keybinding is replaced on mac", macDefault.kind === "NoMatchingKb");
  const winRegistry = createKeybindingsRegistry("windows");
  winRegistry.registerKeybindingRule({ id: "editor.save", keybinding: "ctrl+s", mac: "cmd+shift+s" });
  const winOutcome = winRegistry.getResolver().resolve(new ContextKeyService(), [chord(true, false, false, false, "s")]);
  check("the default keybinding binds off mac", winOutcome.kind === "KbFound" && winOutcome.commandId === "editor.save");
}
{
  const linuxRegistry = createKeybindingsRegistry("linux");
  linuxRegistry.registerKeybindingRule({ id: "workbench.open", keybinding: "ctrl+p", linux: "ctrl+alt+p" });
  const outcome = linuxRegistry.getResolver().resolve(new ContextKeyService(), [chord(true, false, true, false, "p")]);
  check("the linux override binds on linux", outcome.kind === "KbFound" && outcome.commandId === "workbench.open");
}
{
  // ctrlcmd flows through the registry per platform.
  const macRegistry = createKeybindingsRegistry("mac");
  macRegistry.registerKeybindingRule({ id: "editor.save", keybinding: "ctrlcmd+s" });
  const outcome = macRegistry.getResolver().resolve(new ContextKeyService(), [chord(false, false, false, true, "s")]);
  check("ctrlcmd binds meta on mac through the registry", outcome.kind === "KbFound");
}
{
  const registry = createKeybindingsRegistry("windows");
  registry.registerKeybindingRule({ id: "chrome.resetZoom", keybinding: "ctrl+numpad0" });
  registry.registerKeybindingRule({ id: "chrome.resetZoom", keybinding: "ctrl+0" });
  const label = registry.lookupKeybinding("chrome.resetZoom");
  check("lookupKeybinding labels the first registered rule", label !== undefined && label.getLabel() === "Ctrl+NumPad0");
  check("lookupKeybinding is undefined for an unknown command", registry.lookupKeybinding("nope") === undefined);
}
{
  const macRegistry = createKeybindingsRegistry("mac");
  macRegistry.registerKeybindingRule({ id: "editor.save", keybinding: "ctrlcmd+s" });
  const label = macRegistry.lookupKeybinding("editor.save");
  check("the label renders per platform", label !== undefined && label.getLabel() === "Cmd+S");
}
{
  // Malformed strings are reported once at registration and never match.
  const errors = [];
  const originalError = console.error;
  console.error = (message) => errors.push(String(message));
  try {
    const registry = createKeybindingsRegistry("windows");
    registry.registerKeybindingRule({ id: "bad.chord", keybinding: "ctrl+banana" });
    registry.registerKeybindingRule({ id: "bad.when", keybinding: "ctrl+s", when: "a &&" });
    check("a malformed keybinding is reported at registration", errors.length === 2);
    check("a malformed keybinding never resolves", registry.getResolver().resolve(new ContextKeyService(), ctrlM).kind === "NoMatchingKb");
    check("a malformed rule has no label", registry.lookupKeybinding("bad.chord") === undefined);
    check("a malformed when never resolves", registry.getResolver().resolve(new ContextKeyService(), [chord(true, false, false, false, "s")]).kind === "NoMatchingKb");
  } finally {
    console.error = originalError;
  }
}
{
  // A registered when gates resolution through the registry.
  const context = new ContextKeyService();
  const focus = context.createKey("editorTextFocus", false);
  const registry = createKeybindingsRegistry("windows");
  registry.registerKeybindingRule({ id: "editor.save", keybinding: "ctrl+s", when: "editorTextFocus" });
  check("a failing when gates a registry rule", registry.getResolver().resolve(context, [chord(true, false, false, false, "s")]).kind === "NoMatchingKb");
  focus.set(true);
  check("a passing when releases a registry rule", registry.getResolver().resolve(context, [chord(true, false, false, false, "s")]).kind === "KbFound");
  context.dispose();
}
{
  // The module-level singleton shares the detect-platform behavior.
  const registration = KeybindingsRegistry.registerKeybindingRule({ id: "test.singleton", keybinding: "f9" });
  const outcome = KeybindingsRegistry.getResolver().resolve(new ContextKeyService(), [chord(false, false, false, false, "f9")]);
  check("the KeybindingsRegistry singleton resolves rules", outcome.kind === "KbFound" && outcome.commandId === "test.singleton");
  registration.dispose();
}

if (failures.length > 0) {
  console.error(`keybindings: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("keybindings: all assertions passed");
process.exit(0);
