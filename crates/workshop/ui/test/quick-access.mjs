// Unit test for the quick-access registry
// (src/services/quick-access-registry.ts): longest-prefix routing of an
// input value to its provider, upsert-by-prefix registration with
// disposables that unregister only their own registration, and the
// provider listing that feeds the ? help list. Bundles the module with
// esbuild and drives it.
// Run: node --test test/quick-access.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { QuickAccessRegistry, createQuickAccessRegistry } from "./src/services/quick-access-registry.ts";
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
const { QuickAccessRegistry, createQuickAccessRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function provider(prefix, options = {}) {
  return {
    prefix,
    placeholder: options.placeholder ?? `placeholder for ${prefix}`,
    helpEntries: options.helpEntries ?? [{ description: `help for ${prefix}`, prefix }],
    factory: options.factory ?? (() => ({ tag: prefix })),
  };
}

// --- Longest-prefix routing ---------------------------------------------------

{
  const registry = createQuickAccessRegistry();
  check("an empty registry routes nothing", registry.getQuickAccessProvider("anything") === undefined);
}
{
  const registry = createQuickAccessRegistry();
  registry.registerQuickAccessProvider(provider(""));
  registry.registerQuickAccessProvider(provider(">"));
  registry.registerQuickAccessProvider(provider(":"));
  check("the empty prefix is the default route", registry.getQuickAccessProvider("file.ts")?.prefix === "");
  check("a bare input routes to the default provider", registry.getQuickAccessProvider("")?.prefix === "");
  check("a prefix routes to its provider", registry.getQuickAccessProvider(">save")?.prefix === ">");
  check("another prefix routes to its provider", registry.getQuickAccessProvider(":42")?.prefix === ":");
  check("the bare prefix still routes", registry.getQuickAccessProvider(">")?.prefix === ">");
}
{
  // Longest matching prefix wins over shorter ones that also match.
  const registry = createQuickAccessRegistry();
  registry.registerQuickAccessProvider(provider(""));
  registry.registerQuickAccessProvider(provider("debug"));
  registry.registerQuickAccessProvider(provider("debug "));
  check("the longest matching prefix wins", registry.getQuickAccessProvider("debug launch")?.prefix === "debug ");
  check("a shorter match serves its own values", registry.getQuickAccessProvider("debugger")?.prefix === "debug");
}
{
  // Registration order does not decide the route; prefix length does.
  const registry = createQuickAccessRegistry();
  registry.registerQuickAccessProvider(provider("debug "));
  registry.registerQuickAccessProvider(provider(""));
  registry.registerQuickAccessProvider(provider("debug"));
  check("length beats registration order", registry.getQuickAccessProvider("debug x")?.prefix === "debug ");
}
{
  // Without an empty-prefix provider, unrouted values get nothing.
  const registry = createQuickAccessRegistry();
  registry.registerQuickAccessProvider(provider(">"));
  check("no default provider means no route", registry.getQuickAccessProvider("file.ts") === undefined);
}

// --- Upsert by prefix -----------------------------------------------------------

{
  const registry = createQuickAccessRegistry();
  const first = registry.registerQuickAccessProvider(provider(">", { placeholder: "first" }));
  registry.registerQuickAccessProvider(provider(">", { placeholder: "second" }));
  check("a second registration replaces the first", registry.getQuickAccessProvider(">x")?.placeholder === "second");
  first.dispose();
  check("disposing the replaced registration keeps the replacement", registry.getQuickAccessProvider(">x")?.placeholder === "second");
}

// --- Disposal -------------------------------------------------------------------

{
  const registry = createQuickAccessRegistry();
  const registration = registry.registerQuickAccessProvider(provider("%"));
  check("a registered provider routes", registry.getQuickAccessProvider("%x")?.prefix === "%");
  registration.dispose();
  check("disposing unregisters the provider", registry.getQuickAccessProvider("%x") === undefined);
  registration.dispose();
  check("double disposal is harmless", registry.getQuickAccessProvider("%x") === undefined);
}
{
  // Disposing one provider leaves the others routing.
  const registry = createQuickAccessRegistry();
  const files = registry.registerQuickAccessProvider(provider(""));
  registry.registerQuickAccessProvider(provider(">"));
  files.dispose();
  check("disposing the default leaves prefixed routes", registry.getQuickAccessProvider(">x")?.prefix === ">");
  check("disposing the default removes the default route", registry.getQuickAccessProvider("x") === undefined);
}

// --- The ? help list ---------------------------------------------------------------

{
  const registry = createQuickAccessRegistry();
  registry.registerQuickAccessProvider(provider("", { helpEntries: [{ description: "Go to File", prefix: "" }] }));
  registry.registerQuickAccessProvider(provider(">", { helpEntries: [{ description: "Show and Run Commands", prefix: ">" }] }));
  registry.registerQuickAccessProvider(provider("?", { helpEntries: [{ description: "Show Help", prefix: "?" }] }));
  const providers = registry.getQuickAccessProviders();
  check("the listing includes every provider", providers.length === 3);
  check(
    "the listing includes help entries",
    providers.some((entry) => entry.helpEntries.some((help) => help.description === "Show and Run Commands" && help.prefix === ">")),
  );
  check(
    "the listing includes placeholders",
    providers.every((entry) => typeof entry.placeholder === "string" && entry.placeholder.length > 0),
  );
  const stale = registry.registerQuickAccessProvider(provider("@"));
  stale.dispose();
  check("the listing drops disposed providers", registry.getQuickAccessProviders().length === 3);
}

// --- The module-level singleton -----------------------------------------------------

{
  const registration = QuickAccessRegistry.registerQuickAccessProvider(provider("test-singleton:"));
  check("the QuickAccessRegistry singleton routes", QuickAccessRegistry.getQuickAccessProvider("test-singleton:x")?.prefix === "test-singleton:");
  registration.dispose();
  check("the singleton unregisters on dispose", QuickAccessRegistry.getQuickAccessProvider("test-singleton:x") === undefined);
}

if (failures.length > 0) {
  console.error(`quick-access: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("quick-access: all assertions passed");
process.exit(0);
