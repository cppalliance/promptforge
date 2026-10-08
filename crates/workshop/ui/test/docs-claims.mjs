// Guard for the workspace-debt removal (plan step 7): the repository's
// prose describes the shipped behavior. Each phrase below was once true
// and is now stale, so its return would mean a doc was reverted or a
// paragraph copied from history. Reads the files with node:fs rather
// than importing anything, so the check needs no bundle and runs in the
// plain SPA suite.
// Run: node --test test/docs-claims.mjs
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

/** Every line of `file` (relative to the repo root) matching `phrase`, a string or a RegExp, tagged with its number. */
async function offendingLines(file, phrase) {
  const text = await readFile(path.join(repoRoot, file), "utf8");
  return text
    .split("\n")
    .map((line, index) => ({ line, number: index + 1 }))
    .filter(({ line }) => (phrase instanceof RegExp ? phrase.test(line) : line.includes(phrase)))
    .map(({ line, number }) => `${file}:${number}: ${line.trim()}`);
}

test("AGENTS.md names two UI-state homes, not a TOML config or three buckets", async () => {
  // UM-002: the SPA rule once routed "account preferences" to a
  // "machine-written TOML config" that no code, route, or allow-list
  // implements, and counted three buckets where two exist.
  for (const phrase of ["TOML config", "three named buckets", "three homes"]) {
    assert.deepEqual(await offendingLines("AGENTS.md", phrase), [], `AGENTS.md still says "${phrase}"`);
  }
});

// Guard for the Engine, Harness, Host, and Plugin definitions in the root AGENTS.md.
// Coding sessions read the rulebooks (every AGENTS.md, every .cursor/rules
// file, and the crate doc of every lib.rs or main.rs that carries
// `## Invariants`) as instructions, so each one uses the four words one way.
// Inline and fenced code is skipped, and so is the Definitions section itself,
// whose rule text names the words it restricts.
const SKIP_DIRS = new Set([".git", "node_modules", "target", "dist"]);
// Network and outside-tool phrases that keep a lowercase "host".
const HOST_ALLOWED = /host-and-address|self-hosted|github-hosted/gi;
// Other senses that keep a lowercase "engine" or "harness".
const TERM_ALLOWED = /speech engine|database engine|test harness/gi;
// Other programs' plugins, which keep a lowercase "plugin".
const PLUGIN_ALLOWED = /Tauri plugin|ProseMirror plugin|esbuild plugin|NSIS plugin/gi;
// Senses of "capability" other than the Plugin.
const CAPABILITY_ALLOWED = /access capability|Tauri capability|model capabilities/gi;
const RETIRED = ["production host", "engine's host", "hosts the engine", "harness that hosts", "host globals", "host-support"];

async function* walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) {
      yield path.join(dir, entry.name);
    } else if (!SKIP_DIRS.has(entry.name)) {
      yield* walk(path.join(dir, entry.name));
    }
  }
}

/** Every prose line of every rulebook, with inline code blanked, as `{ file, number, text }`. */
async function rulebookLines() {
  const out = [];
  for await (const full of walk(repoRoot)) {
    const file = path.relative(repoRoot, full).split(path.sep).join("/");
    const base = path.basename(file);
    let lines;
    if (base === "AGENTS.md" || (file.startsWith(".cursor/rules/") && base.endsWith(".mdc"))) {
      lines = (await readFile(full, "utf8")).split("\n").map((text, index) => ({ text, number: index + 1 }));
    } else if (base === "lib.rs" || base === "main.rs") {
      const doc = (await readFile(full, "utf8"))
        .split("\n")
        .map((text, index) => ({ text, number: index + 1 }))
        .filter(({ text }) => text.trimStart().startsWith("//!"));
      if (!doc.some(({ text }) => text.includes("## Invariants"))) {
        continue;
      }
      lines = doc.map(({ text, number }) => ({ text: text.trimStart().slice(3), number }));
    } else {
      continue;
    }
    let fenced = false;
    let definitions = false;
    for (const { text, number } of lines) {
      if (text.trimStart().startsWith("```")) {
        fenced = !fenced;
        continue;
      }
      if (file === "AGENTS.md" && text.startsWith("## ")) {
        definitions = text.startsWith("## Definitions");
      }
      if (!fenced && !definitions) {
        out.push({ file, number, text: text.replace(/`[^`]*`/g, " ") });
      }
    }
  }
  return out;
}

test("rulebooks use Engine, Harness, Host, and Plugin as the root AGENTS.md defines them", async () => {
  const offenders = [];
  for (const { file, number, text } of await rulebookLines()) {
    const where = `${file}:${number}: ${text.trim()}`;
    for (const phrase of RETIRED) {
      if (text.toLowerCase().includes(phrase)) {
        offenders.push(`${where} (retired phrase "${phrase}")`);
      }
    }
    if (/\bhost(s|ed|ing)?\b|\bhost-/.test(text.replace(HOST_ALLOWED, " "))) {
      offenders.push(`${where} (lowercase "host": required "Host" for the application, another word for any other sense)`);
    }
    const terms = file.startsWith("crates/gateway/stt/") ? /(?<![-_/:\w])harness(?![-_/:\w])/ : /(?<![-_/:\w])(engine|harness)(?![-_/:\w])/;
    if (terms.test(text.replace(TERM_ALLOWED, " "))) {
      offenders.push(`${where} (lowercase "engine" or "harness": required "Engine" or "Harness" for the defined term)`);
    }
    if (/(?<![-_/:\w])plugins?(?![-_/:\w])/.test(text.replace(PLUGIN_ALLOWED, " "))) {
      offenders.push(`${where} (lowercase "plugin": required "Plugin" for the defined term, or the other program's name, such as "Tauri plugin")`);
    }
    if (/capabilit/i.test(text.replace(CAPABILITY_ALLOWED, " "))) {
      offenders.push(`${where} ("capability": the Plugin is never called a capability; another sense names what it grants, such as "access capability")`);
    }
  }
  assert.deepEqual(offenders, [], "rulebook lines break the root AGENTS.md Definitions");
});

// Guard for the Engine rule in the root AGENTS.md: the promptforge crates call
// the code that steps a run "the caller". Coding sessions read Engine prose,
// strings, and code names as design decisions, so naming the Host, a Host
// application, or a Harness crate, type, or internal there steers them to build
// against the wrong component. Every line of every text file is scanned, code
// and strings included. The plain word "Harness" stays allowed for statements
// that a responsibility belongs to the Harness, which the AGENTS.md rule
// governs; a pattern cannot tell those apart.
const ENGINE_ROOTS = ["crates/promptforge", "crates/promptforge-internal"];
const ENGINE_TEXT = /\.(rs|md|lua|toml|txt)$/;
// Externally defined senses that keep a lowercase "harness".
const ENGINE_HARNESS_ALLOWED = /harness = false|test harness/g;
const ENGINE_RULES = [
  [/(?<![A-Za-z])host(s|ed|ing)?(?![A-Za-z])/i, 'names the Host: Engine crates say "the caller" or "application state"'],
  [/\b(workshop|papergate)\b/i, "names a Host application"],
  [/(?<![A-Za-z])harness(?![A-Za-z])/, 'lowercase "harness": a Harness crate, type path, or code name'],
  [/Harness-/, '"Harness-" compound: Engine crates say "caller-supplied" and the like'],
  [/RunHarness/, "the old test-bundle name: the fixture is RunFixture"],
];

test("Engine crates name the caller, never the Host or the Harness's crates and types", async () => {
  const offenders = [];
  for (const root of ENGINE_ROOTS) {
    for await (const full of walk(path.join(repoRoot, root))) {
      if (!ENGINE_TEXT.test(full)) {
        continue;
      }
      const file = path.relative(repoRoot, full).split(path.sep).join("/");
      const lines = (await readFile(full, "utf8")).split("\n");
      for (const [index, text] of lines.entries()) {
        const scanned = text.replace(ENGINE_HARNESS_ALLOWED, " ");
        for (const [pattern, reason] of ENGINE_RULES) {
          if (pattern.test(scanned)) {
            offenders.push(`${file}:${index + 1}: ${text.trim()} (${reason})`);
          }
        }
      }
    }
  }
  assert.deepEqual(offenders, [], "Engine crate lines break the root AGENTS.md Engine rule");
});

// A Host installs each Plugin once, and every run receives every usable
// Plugin's tools, so nothing activates a Plugin when a run starts. Comment
// lines are checked, not code, so a local variable keeps its name.
const PLUGIN_DOC_DIRS = [
  "crates/promptforge",
  "crates/promptforge-internal",
  "crates/promptforge-plugin",
  "crates/harness",
  "crates/harness-internal",
  "crates/harness-gateway-client",
  "crates/plugin-web",
  "crates/plugin-user-input",
  "crates/plugin-mcp",
];

test("the root AGENTS.md and the Plugin-facing crates' comments never describe Plugin activation", async () => {
  const offenders = await offendingLines("AGENTS.md", /activat/i);
  for (const dir of PLUGIN_DOC_DIRS) {
    for await (const full of walk(path.join(repoRoot, dir))) {
      if (!full.endsWith(".rs")) {
        continue;
      }
      const file = path.relative(repoRoot, full).split(path.sep).join("/");
      (await readFile(full, "utf8")).split("\n").forEach((line, index) => {
        if (line.trimStart().startsWith("//") && /activat/i.test(line)) {
          offenders.push(`${file}:${index + 1}: ${line.trim()}`);
        }
      });
    }
  }
  assert.deepEqual(offenders, [], "a doc still says Plugins are activated; say installed, snapshotted, or declared");
});
