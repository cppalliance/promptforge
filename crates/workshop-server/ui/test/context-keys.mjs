// Unit test for the context-key expression parser and service
// (src/services/context-key-expr.ts, src/services/context-key-service.ts):
// the `when` expression language (key, !key, == / != against a string
// literal, &&, ||, parentheses, true, false) parsed into an evaluable
// expression that names the keys it reads, and the service that holds
// key values, binds keys with defaults, evaluates expressions, and fires
// a change event whose affectsSome lets subscribers re-evaluate only the
// expressions that read the changed keys. Bundles the modules with
// esbuild and drives them. Covers: the expression subset, precedence and
// parentheses, keys() collection, parse failures returned as Result
// values (never thrown), createKey defaults visible through getValue,
// set/get/reset, and affectsSome on the change event.
// Run: node --test test/context-keys.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { ContextKeyExpr } from "./src/services/context-key-expr.ts";
      export { ContextKeyService, CONTEXT_KEY_SERVICE } from "./src/services/context-key-service.ts";
      export { getService } from "./src/services/service-registry.ts";
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
const { ContextKeyExpr, ContextKeyService, CONTEXT_KEY_SERVICE, getService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function evaluate(when, values) {
  const result = ContextKeyExpr.deserialize(when);
  if (!result.ok) return `parse-error: ${result.error.message}`;
  return result.value.evaluate((key) => values[key]);
}

// --- Self-registration --------------------------------------------------------

const shared = getService(CONTEXT_KEY_SERVICE);
check("the CONTEXT_KEY_SERVICE token self-registers", shared instanceof ContextKeyService);
check("the registry caches one instance", getService(CONTEXT_KEY_SERVICE) === shared);

// --- Expression subset ----------------------------------------------------------

check("a bare key is truthy", evaluate("editorTextFocus", { editorTextFocus: true }) === true);
check("a bare key fails on falsy", evaluate("editorTextFocus", { editorTextFocus: false }) === false);
check("an unknown key is falsy", evaluate("editorTextFocus", {}) === false);
check("! negates", evaluate("!editorTextFocus", { editorTextFocus: false }) === true);
check("!! is a double negation", evaluate("!!a", { a: 1 }) === true);
check("== matches a string literal", evaluate("editorLangId == 'rust'", { editorLangId: "rust" }) === true);
check("== rejects a different value", evaluate("editorLangId == 'rust'", { editorLangId: "json" }) === false);
check("== fails when the key is unset", evaluate("editorLangId == 'rust'", {}) === false);
check("!= matches a different value", evaluate("editorLangId != 'rust'", { editorLangId: "json" }) === true);
check("!= rejects an equal value", evaluate("editorLangId != 'rust'", { editorLangId: "rust" }) === false);
check("true is true", evaluate("true", {}) === true);
check("false is false", evaluate("false", {}) === false);
check("&& needs both sides", evaluate("a && b", { a: true, b: false }) === false);
check("|| needs one side", evaluate("a || b", { a: false, b: true }) === true);
check("&& binds tighter than ||", evaluate("a || b && c", { a: false, b: true, c: true }) === true);
check("parentheses override precedence", evaluate("(a || b) && c", { a: true, b: false, c: false }) === false);
check(
  "a compound expression evaluates",
  evaluate("editorTextFocus && editorLangId == 'rust' || !inputFocus", {
    editorTextFocus: true,
    editorLangId: "rust",
    inputFocus: true,
  }) === true,
);
check("whitespace is insignificant", evaluate("  a   &&\n\tb  ", { a: true, b: true }) === true);
check("dotted and dashed keys parse", evaluate("config.editor.wordWrap == 'on'", { "config.editor.wordWrap": "on" }) === true);

// --- keys() ---------------------------------------------------------------------

{
  const result = ContextKeyExpr.deserialize("a && !b || c == 'x'");
  check("deserialize succeeds for a compound expression", result.ok);
  const keys = result.ok ? [...result.value.keys()].sort() : [];
  check("keys() names every referenced key", keys.join(",") === "a,b,c");
  const noKeys = ContextKeyExpr.deserialize("true");
  check("a literal references no keys", noKeys.ok && noKeys.value.keys().size === 0);
  const repeated = ContextKeyExpr.deserialize("a && a");
  check("a repeated key is collected once", repeated.ok && repeated.value.keys().size === 1);
}

// --- Parse errors as values -------------------------------------------------------

{
  const empty = ContextKeyExpr.deserialize("");
  check("an empty string is a parse error, not a throw", !empty.ok && typeof empty.error.message === "string");
  const dangling = ContextKeyExpr.deserialize("a &&");
  check("a dangling operator is a parse error", !dangling.ok);
  const unquoted = ContextKeyExpr.deserialize("a == foo");
  check("an unquoted comparison value is a parse error", !unquoted.ok);
  const unclosed = ContextKeyExpr.deserialize("(a || b");
  check("an unclosed parenthesis is a parse error", !unclosed.ok);
  const singleAmp = ContextKeyExpr.deserialize("a & b");
  check("a single & is a parse error", !singleAmp.ok);
  const trailing = ContextKeyExpr.deserialize("a b");
  check("trailing input is a parse error", !trailing.ok);
  const bare = ContextKeyExpr.deserialize("==");
  check("an operator with no key is a parse error", !bare.ok);
  const withOffset = ContextKeyExpr.deserialize("a && )");
  check("a parse error carries a numeric offset", !withOffset.ok && typeof withOffset.error.offset === "number");
}

// --- The service -------------------------------------------------------------------

const service = new ContextKeyService();

check("an unbound key reads undefined", service.getValue("nope") === undefined);
check("contextMatchesRules(undefined) is true", service.contextMatchesRules(undefined) === true);
check("a malformed when string never matches", service.contextMatchesRules("a &&") === false);

const editorTextFocus = service.createKey("editorTextFocus", false);
check("createKey's default is visible through getValue", service.getValue("editorTextFocus") === false);
check("the key's get returns the default", editorTextFocus.get() === false);
check("contextMatchesRules evaluates a string against the service", service.contextMatchesRules("editorTextFocus") === false);
check("contextMatchesRules honors the default", service.contextMatchesRules("!editorTextFocus") === true);

let lastEvent = null;
let fires = 0;
const subscription = service.onDidChangeContext((event) => {
  fires += 1;
  lastEvent = event;
});

editorTextFocus.set(true);
check("set stores the value", editorTextFocus.get() === true);
check("set fires the change event", fires === 1);
check(
  "affectsSome is true for the changed key",
  lastEvent !== null && lastEvent.affectsSome(new Set(["editorTextFocus", "other"])) === true,
);
check(
  "affectsSome is false for untouched keys",
  lastEvent !== null && lastEvent.affectsSome(new Set(["inputFocus"])) === false,
);
check("the new value flips evaluation", service.contextMatchesRules("editorTextFocus") === true);

editorTextFocus.reset();
check("reset restores the default", editorTextFocus.get() === false);
check("reset fires the change event", fires === 2);

{
  const parsed = ContextKeyExpr.deserialize("editorTextFocus && editorLangId == 'rust'");
  check("the compound expression parses", parsed.ok);
  check("an expression object evaluates against the service", parsed.ok && service.contextMatchesRules(parsed.value) === false);
  const langId = service.createKey("editorLangId", "json");
  langId.set("rust");
  editorTextFocus.set(true);
  check("evaluation follows later sets", parsed.ok && service.contextMatchesRules(parsed.value) === true);
}

const firesBeforeDispose = fires;
subscription.dispose();
editorTextFocus.set(false);
check("disposing the subscription stops events", fires === firesBeforeDispose);

service.dispose();
shared.dispose();

if (failures.length > 0) {
  console.error(`context-keys: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("context-keys: all assertions passed");
process.exit(0);
