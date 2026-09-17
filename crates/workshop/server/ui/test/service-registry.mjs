// Unit test for the service registry (src/services/service-registry.ts):
// the token-keyed registry every subsystem self-registers into. Bundles
// the module with esbuild and drives the public API. Covers: lookup of an
// unregistered token (null from getServiceOrNull, a naming throw from
// getService), lazy instantiation on first get with caching afterwards,
// replacement semantics when a token is re-registered (the stale cached
// instance is dropped), unregistering through the registration's
// disposable, and independence between tokens.
// Run: node test/service-registry.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "services", "service-registry.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { createServiceToken, registerService, getService, getServiceOrNull } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Unregistered tokens ----------------------------------------------------

const MISSING = createServiceToken("test.missing");
check("getServiceOrNull answers null for an unregistered token", getServiceOrNull(MISSING) === null);
let threw = null;
try {
  getService(MISSING);
} catch (error) {
  threw = error;
}
check(
  "getService throws and names the token for an unregistered token",
  threw !== null && threw.message.includes("test.missing"),
);

// --- Lazy instantiation and caching ------------------------------------------

const CACHED = createServiceToken("test.cached");
let builds = 0;
registerService(CACHED, () => {
  builds += 1;
  return { serial: builds };
});
check("the factory does not run at registration time", builds === 0);
const first = getService(CACHED);
check("the first get instantiates through the factory", builds === 1 && first.serial === 1);
check("the instance is cached across gets", getService(CACHED) === first && builds === 1);

// --- Replacement drops the stale cached instance ------------------------------

registerService(CACHED, () => {
  builds += 1;
  return { serial: builds };
});
const replaced = getService(CACHED);
check(
  "re-registering replaces the factory and drops the cached instance",
  replaced !== first && replaced.serial === 2 && builds === 2,
);

// --- The registration disposable unregisters ---------------------------------

const REMOVABLE = createServiceToken("test.removable");
const registration = registerService(REMOVABLE, () => ({}));
check("a registered token resolves", getServiceOrNull(REMOVABLE) !== null);
registration.dispose();
check("disposing the registration unregisters the token", getServiceOrNull(REMOVABLE) === null);

// --- Tokens are independent ----------------------------------------------------

const A = createServiceToken("test.a");
const B = createServiceToken("test.b");
registerService(A, () => "a");
registerService(B, () => "b");
check("tokens resolve their own instances", getService(A) === "a" && getService(B) === "b");

if (failures.length > 0) {
  console.error(`service-registry: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("service-registry: all assertions passed");
process.exit(0);
