// Unit test for the layer rule (check-layers.mjs checkImport). Imports the
// rule module directly - it is dependency-free plain ESM, so no bundling is
// needed - and pins the allow/deny matrix: base may import only base;
// services may import base and services; ui may import everything
// but the composition root; main.ts may import every layer;
// nothing may import main.ts; a file
// in no layer is flagged from either side. If checkImport regressed to
// allow a reverse import, this test fails even though the conforming tree
// keeps every wired walk green.
// Run: node test/check-layers.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";

import { checkImport } from "../check-layers.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const srcDir = path.join(uiDir, "..", "src");
const at = (...parts) => path.resolve(srcDir, ...parts);

const failures = [];
function allowed(name, importer, imported) {
  const violation = checkImport(importer, imported);
  if (violation !== null) failures.push(`${name}: unexpectedly denied (${violation})`);
}
function denied(name, importer, imported) {
  if (checkImport(importer, imported) === null) failures.push(`${name}: unexpectedly allowed`);
}

// --- Imports the rule allows ---------------------------------------------------

allowed("base imports base", at("base", "lifecycle.ts"), at("base", "event.ts"));
allowed("services imports base", at("services", "model-service.ts"), at("base", "event.ts"));
allowed(
  "services imports services",
  at("services", "workshop-socket.ts"),
  at("services", "protocol.ts"),
);
allowed("ui imports base", at("ui", "status-bar.ts"), at("base", "lifecycle.ts"));
allowed("ui imports services", at("ui", "status-bar.ts"), at("services", "protocol.ts"));
allowed("ui imports ui", at("ui", "workshop", "zones.ts"), at("ui", "workshop", "panel-types.ts"));
allowed("main.ts imports base", at("main.ts"), at("base", "lifecycle.ts"));
allowed("main.ts imports services", at("main.ts"), at("services", "workshop-socket.ts"));
allowed("main.ts imports ui", at("main.ts"), at("ui", "window-chrome.ts"));
allowed(
  "an extensionless resolution classifies by its directory",
  at("services", "model-service.ts"),
  at("base", "event"),
);

// --- Imports the rule denies -----------------------------------------------------

denied("base may not import services", at("base", "lifecycle.ts"), at("services", "protocol.ts"));
denied("base may not import ui", at("base", "lifecycle.ts"), at("ui", "status-bar.ts"));
// The vendored chat/ tree was deleted with the murm-ui removal; a re-grown
// chat/ directory sits in no layer until it is deliberately re-sanctioned.
denied(
  "the deleted chat/ tree is no longer a layer",
  at("ui", "status-bar.ts"),
  at("chat", "core", "types.ts"),
);
denied(
  "services may not import ui",
  at("services", "workshop-socket.ts"),
  at("ui", "status-bar.ts"),
);
denied("services may not import main.ts", at("services", "workshop-socket.ts"), at("main.ts"));
denied("ui may not import main.ts", at("ui", "window-menu.ts"), at("main.ts"));
denied(
  "ui may not import an extensionless main",
  at("ui", "window-menu.ts"),
  at("main"),
);
denied(
  "a src-root file sits in no layer",
  at("stray.ts"),
  at("base", "lifecycle.ts"),
);
denied(
  "importing a file outside every layer is flagged",
  at("ui", "status-bar.ts"),
  at("stray.ts"),
);

if (failures.length > 0) {
  console.error(`check-layers test: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("check-layers test: all assertions passed");
