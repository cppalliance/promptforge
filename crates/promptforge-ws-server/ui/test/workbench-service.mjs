// Unit test for the workbench snapshot service
// (src/services/workbench-service.ts). Bundles the TS module with esbuild
// and imports it via a data URL. The server pushes complete workbench
// snapshots; the service holds the last one and fans out changes. Covers:
// the empty pre-boot state, the apply/emit cycle (fields land under UI
// naming, the emitter fires with the held snapshot), every apply firing
// even when unchanged (snapshots are authoritative, not diffed), a later
// apply replacing the held snapshot, and disposed subscriptions receiving
// nothing. Runs under the shared disposable-leak check.
// Run: node test/workbench-service.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { WorkbenchService } from "./src/services/workbench-service.ts";
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
const { lifecycle, WorkbenchService } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function workbenchFrame(overrides = {}) {
  return {
    type: "workbench",
    profiles: ["main", "coding"],
    active: "main",
    switching: null,
    selected: "test-model",
    chat_ready: true,
    ...overrides,
  };
}

await assertNoLeaks(lifecycle, () => {
  // --- The empty pre-boot state -----------------------------------------------

  {
    const service = new WorkbenchService();
    const snapshot = service.snapshot;
    check(
      "before any push the snapshot is empty and chat is gated off",
      snapshot.profiles.length === 0 &&
        snapshot.active === null &&
        snapshot.switching === null &&
        snapshot.selected === null &&
        snapshot.chatReady === false,
    );
    service.dispose();
  }

  // --- The apply/emit cycle ----------------------------------------------------

  {
    const service = new WorkbenchService();
    const emitted = [];
    service.onDidChangeSnapshot((snapshot) => emitted.push(snapshot));
    service.applySnapshot(workbenchFrame());
    check("applying a frame notifies subscribers", emitted.length === 1);
    check(
      "the emitted snapshot is the held snapshot",
      emitted[0] === service.snapshot,
    );
    check(
      "the frame's fields land under UI naming",
      service.snapshot.profiles.join(",") === "main,coding" &&
        service.snapshot.active === "main" &&
        service.snapshot.switching === null &&
        service.snapshot.selected === "test-model" &&
        service.snapshot.chatReady === true,
    );
    service.applySnapshot(workbenchFrame());
    check(
      "an unchanged frame still notifies - snapshots are authoritative",
      emitted.length === 2,
    );
    service.applySnapshot(
      workbenchFrame({ switching: "coding", selected: null, chat_ready: false }),
    );
    check(
      "a later frame replaces the held snapshot",
      service.snapshot.switching === "coding" &&
        service.snapshot.selected === null &&
        service.snapshot.chatReady === false,
    );
    service.dispose();
  }

  // --- Disposed subscriptions receive nothing -----------------------------------

  {
    const service = new WorkbenchService();
    const emitted = [];
    const subscription = service.onDidChangeSnapshot((snapshot) => emitted.push(snapshot));
    service.applySnapshot(workbenchFrame());
    subscription.dispose();
    service.applySnapshot(workbenchFrame({ active: "coding" }));
    check("a disposed subscription receives nothing", emitted.length === 1);
    check("state still updates after subscribers leave", service.snapshot.active === "coding");
    service.dispose();
  }
});

if (failures.length > 0) {
  console.error(`workbench-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workbench-service: all assertions passed");
