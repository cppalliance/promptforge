// Unit test for the zone-state service (src/services/zone-state-service.ts):
// the zone group map and per-panel placement overrides, moved out of
// zones.ts module scope into a registry-held service so the state is
// named, observable, and shared through the service registry. Bundles the
// module with esbuild and drives it. Covers: self-registration (the
// ZONE_STATE token resolves without any composition-root wiring), the
// singleton surviving across lookups, group id round-trips and reverse
// lookup, override set/clear, serialize/restore/reset with validation of
// persisted shapes, and the change event firing on override writes.
// Run: node test/zone-state-service.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { ZoneStateService, ZONE_STATE, ZONE_NAMES } from "./src/services/zone-state-service.ts";
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
const { ZoneStateService, ZONE_STATE, ZONE_NAMES, getService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// --- Self-registration and the singleton ------------------------------------

const service = getService(ZONE_STATE);
check("the ZONE_STATE token self-registers", service instanceof ZoneStateService);
check("the registry caches one instance", getService(ZONE_STATE) === service);
check("the declared zones are left, main, right", ZONE_NAMES.join(",") === "left,main,right");

// --- Group ids ----------------------------------------------------------------

service.reset();
check("an unknown zone has no group", service.groupFor("left") === undefined);
service.setGroup("left", "g1");
check("the group id round-trips", service.groupFor("left") === "g1");
check("the reverse lookup names the zone", service.zoneForGroupId("g1") === "left");
check("an unknown group id has no zone", service.zoneForGroupId("nope") === undefined);

// --- Overrides ------------------------------------------------------------------

let changes = 0;
const subscription = service.onDidChange(() => {
  changes += 1;
});
check("a panel without an override has none", service.overrideFor("editor:x") === undefined);
service.setOverride("editor:x", "right");
check("the override round-trips", service.overrideFor("editor:x") === "right");
check("setting an override fires the change event", changes === 1);
service.clearOverride("editor:x");
check("clearing removes the override", service.overrideFor("editor:x") === undefined);
check("clearing fires the change event", changes === 2);
subscription.dispose();

// --- Serialize / restore / reset -------------------------------------------------

service.setGroup("main", "g9");
service.setOverride("agent", "main");
const snapshot = service.serialize();
check(
  "serialize carries the zones and overrides records",
  snapshot.zones.main === "g9" && snapshot.overrides.agent === "main",
);

const restored = new ZoneStateService();
restored.restore(snapshot.zones, snapshot.overrides);
check(
  "restore round-trips the state",
  restored.groupFor("main") === "g9" && restored.overrideFor("agent") === "main",
);
restored.restore({ left: 42, bogus: "g" }, { "editor:x": "nowhere", "editor:y": "left" });
check(
  "restore drops unknown zones and non-string values",
  restored.groupFor("left") === undefined &&
    restored.groupFor("bogus") === undefined &&
    restored.overrideFor("editor:x") === undefined &&
    restored.overrideFor("editor:y") === "left",
);
restored.reset();
check(
  "reset clears groups and overrides",
  restored.groupFor("main") === undefined && restored.overrideFor("agent") === undefined,
);

service.dispose();

if (failures.length > 0) {
  console.error(`zone-state-service: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("zone-state-service: all assertions passed");
process.exit(0);
