// Covering test for the shared leak-check helper
// (test/helpers/leak-check.mjs). Bundles src/base/lifecycle.ts with
// esbuild and imports it via a data URL, then drives assertNoLeaks
// through both verdicts. Covers: a clean sync run passes, a clean async
// run through a Disposable subclass passes, a leaking run throws with
// the leak count and the construction site named (bare store and
// Disposable subclass), the run's own error propagates, and a store
// created between runs is not attributed to a later run.
// Run: node test/leak-check.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

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

const lifecycle = await loadModule(path.join("base", "lifecycle.ts"));
const { Disposable, DisposableStore } = lifecycle;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Clean runs pass ----------------------------------------------------------

{
  let error = null;
  try {
    await assertNoLeaks(lifecycle, () => {
      const store = new DisposableStore();
      store.add({ dispose() {} });
      store.dispose();
    });
  } catch (caught) {
    error = caught;
  }
  check("a clean sync run passes", error === null);
}

{
  class CleanComponent extends Disposable {}
  let error = null;
  try {
    await assertNoLeaks(lifecycle, async () => {
      const component = new CleanComponent();
      await Promise.resolve();
      component.dispose();
    });
  } catch (caught) {
    error = caught;
  }
  check("a clean async run through a Disposable subclass passes", error === null);
}

// --- Leaking runs fail, naming the construction site ---------------------------

{
  function leakyComponent() {
    new DisposableStore(); // never disposed
  }
  let error = null;
  try {
    await assertNoLeaks(lifecycle, () => {
      leakyComponent();
    });
  } catch (caught) {
    error = caught;
  }
  check("a leaking run throws", error instanceof Error);
  check(
    "the failure counts the leaked stores",
    error !== null && error.message.includes("1 DisposableStore(s) leaked"),
  );
  check(
    "the failure names the construction site",
    error !== null && error.message.includes("leakyComponent"),
  );
}

{
  class LeakyPanel extends Disposable {}
  let error = null;
  try {
    await assertNoLeaks(lifecycle, () => {
      new LeakyPanel();
    });
  } catch (caught) {
    error = caught;
  }
  check("a leaked Disposable subclass is caught", error instanceof Error);
  check(
    "the subclass leak is named by its class",
    error !== null && error.message.includes("LeakyPanel"),
  );
}

// --- The run's own error propagates and the tracker is uninstalled -------------

{
  let error = null;
  try {
    await assertNoLeaks(lifecycle, () => {
      throw new Error("boom");
    });
  } catch (caught) {
    error = caught;
  }
  check("the run's own error propagates", error !== null && error.message === "boom");

  // A store created between runs must not be attributed to a later run.
  const stray = new DisposableStore();
  let secondError = null;
  try {
    await assertNoLeaks(lifecycle, () => {});
  } catch (caught) {
    secondError = caught;
  }
  check("a run after a throwing run starts clean", secondError === null);
  stray.dispose();
}

if (failures.length > 0) {
  console.error(`leak-check: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("leak-check: all assertions passed");
