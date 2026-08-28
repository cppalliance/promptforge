// Unit test for the shared model service (src/services/model-service.ts).
// Bundles the TS module with esbuild and imports it via a data URL. The
// server owns the selection, so the service splits command from state:
// setCurrent sends the select command through the injected function and
// mutates nothing; applySelected applies the server's snapshot without
// sending. Covers: catalog set/get with no selection fallback, the
// command sending without mutating and reporting the send outcome, the
// apply/emit cycle firing only on real changes, apply never re-sending,
// and disposed subscriptions receiving nothing. Runs under the shared
// disposable-leak check.
// Run: node test/model-service.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { ModelService } from "./src/services/model-service.ts";
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
const code = bundle.outputFiles[0].text;
const { lifecycle, ModelService } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const ids = (models) => models.map((model) => model.id).join(",");

await assertNoLeaks(lifecycle, () => {
  // --- Catalog state: no selection fallback --------------------------------

  {
    const sent = [];
    const service = new ModelService((id) => (sent.push(id), true));
    check("a new service starts with an empty catalog", service.models.length === 0);
    check("a new service starts with no selection", service.current === "");
    service.setModels([{ id: "alpha", description: "the alpha model" }, { id: "beta" }]);
    check("setModels records the catalog", ids(service.models) === "alpha,beta");
    check("a new catalog leaves the selection untouched", service.current === "");
    check("a new catalog sends no select command", sent.length === 0);
    service.dispose();
  }

  // --- setCurrent is a command: sends without mutating ----------------------

  {
    const sent = [];
    const service = new ModelService((id) => (sent.push(id), true));
    const currents = [];
    service.onDidChangeCurrent((id) => currents.push(id));
    check("setCurrent reports a successful send", service.setCurrent("alpha") === true);
    check("setCurrent puts the select command on the wire", sent.join(",") === "alpha");
    check("the command mutates nothing", service.current === "");
    check("the command fires no change event", currents.length === 0);
    check(
      "re-issuing the command sends again - the server owns dedupe",
      service.setCurrent("alpha") === true && sent.join(",") === "alpha,alpha",
    );
    service.dispose();
  }

  {
    const service = new ModelService(() => false);
    check("setCurrent reports a failed send", service.setCurrent("alpha") === false);
    check("a failed send also mutates nothing", service.current === "");
    service.dispose();
  }

  // --- applySelected: the apply/emit cycle -----------------------------------

  {
    const sent = [];
    const service = new ModelService((id) => (sent.push(id), true));
    const currents = [];
    service.onDidChangeCurrent((id) => currents.push(id));
    service.applySelected("alpha");
    check("applying a snapshot selection records it", service.current === "alpha");
    check("applying a snapshot selection fires the change event", currents.join("|") === "alpha");
    service.applySelected("alpha");
    check("re-applying the same selection fires nothing", currents.join("|") === "alpha");
    service.applySelected("beta");
    check("a changed selection fires with the new id", currents.join("|") === "alpha|beta");
    service.applySelected(null);
    check("a null selection clears the current model", service.current === "");
    check("clearing fires with the empty id", currents.join("|") === "alpha|beta|");
    check("apply never re-sends, so a snapshot cannot echo", sent.length === 0);
    service.dispose();
  }

  // --- Disposed subscriptions receive nothing ---------------------------------

  {
    const service = new ModelService(() => true);
    const catalogs = [];
    const currents = [];
    const modelsSubscription = service.onDidChangeModels((models) => catalogs.push(ids(models)));
    const currentSubscription = service.onDidChangeCurrent((id) => currents.push(id));
    service.setModels([{ id: "alpha" }]);
    service.applySelected("alpha");
    modelsSubscription.dispose();
    currentSubscription.dispose();
    service.setModels([{ id: "gamma" }]);
    service.applySelected("gamma");
    check("a disposed catalog subscription receives nothing", catalogs.join("|") === "alpha");
    check("a disposed selection subscription receives nothing", currents.join("|") === "alpha");
    check("state still updates after subscribers leave", service.current === "gamma");
    service.dispose();
  }
});

if (failures.length > 0) {
  console.error(`model-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("model-service: all assertions passed");
