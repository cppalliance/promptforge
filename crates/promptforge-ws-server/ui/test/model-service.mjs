// Unit test for the shared model service (src/services/model-service.ts).
// Bundles the TS module with esbuild and imports it via a data URL.
// Covers: catalog set/get, current-model set/get, selection
// reconciliation on a catalog refresh (kept when it survives, first
// entry when it does not, cleared on an empty catalog), both change
// events firing with their payloads, re-selection firing nothing, and
// disposed subscriptions receiving nothing.
// Run: node test/model-service.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

async function loadModule(relative) {
  const bundle = await esbuild.build({
    entryPoints: [path.join(uiDir, "..", "src", relative)],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    logLevel: "silent",
  });
  const code = bundle.outputFiles[0].text;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const { ModelService } = await loadModule(path.join("services", "model-service.ts"));

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const ids = (models) => models.map((model) => model.id).join(",");

// --- Catalog and current-model state ------------------------------------------

{
  const service = new ModelService();
  check("a new service starts with an empty catalog", service.models.length === 0);
  check("a new service starts with no selection", service.current === "");
  service.setModels([{ id: "alpha", description: "the alpha model" }, { id: "beta" }]);
  check("setModels records the catalog", ids(service.models) === "alpha,beta");
  check("an empty selection falls back to the first entry", service.current === "alpha");
  service.setCurrent("beta");
  check("setCurrent records the selection", service.current === "beta");
}

// --- Selection reconciliation on a catalog refresh ----------------------------

{
  const service = new ModelService();
  service.setModels([{ id: "alpha" }, { id: "beta" }]);
  service.setCurrent("beta");
  service.setModels([{ id: "beta" }, { id: "gamma" }]);
  check("a surviving selection is kept across a refresh", service.current === "beta");
  service.setModels([{ id: "delta" }]);
  check("a dropped selection falls back to the first entry", service.current === "delta");
  service.setModels([]);
  check("an empty catalog clears the selection", service.current === "");
}

// --- Change events: payload delivery and unsubscribe ---------------------------

{
  const service = new ModelService();
  const catalogs = [];
  const currents = [];
  const modelsSubscription = service.onDidChangeModels((models) => catalogs.push(ids(models)));
  const currentSubscription = service.onDidChangeCurrent((id) => currents.push(id));

  service.setModels([{ id: "alpha" }]);
  check("onDidChangeModels fires with the new catalog", catalogs.join("|") === "alpha");
  check("the reconciled selection fires onDidChangeCurrent", currents.join("|") === "alpha");

  service.setCurrent("alpha");
  check("re-selecting the current model fires nothing", currents.join("|") === "alpha");

  service.setCurrent("beta");
  check("onDidChangeCurrent fires with the new selection", currents.join("|") === "alpha|beta");

  modelsSubscription.dispose();
  currentSubscription.dispose();
  service.setModels([{ id: "gamma" }]);
  check("a disposed catalog subscription receives nothing", catalogs.join("|") === "alpha");
  check("a disposed selection subscription receives nothing", currents.join("|") === "alpha|beta");
  check("state still updates after subscribers leave", service.current === "gamma");
}

if (failures.length > 0) {
  console.error(`model-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("model-service: all assertions passed");
