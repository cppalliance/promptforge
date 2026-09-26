// The TS half of the workshop-frame wire contract: every frame in the
// shared fixture crates/workshop/protocol/tests/fixtures/workshop-frames.json
// routes through WorkshopSocket unchanged (server-to-client), and every
// frame the socket sends matches its fixture entry byte-for-byte as parsed
// JSON (client-to-server). The Rust half is the fixture test in
// crates/workshop/protocol/tests/it/workshop_frames.rs; both suites pin the
// same case list, so a wire drift or a case added on one side fails the
// other. Every inbound fixture frame also passes its protocol.ts guard, and
// a copy missing any required field is dropped with one warning.
// Run: node test/workshop-wire-fixtures.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";
import * as esbuild from "esbuild";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { WorkshopSocket } from "./src/services/workshop-socket.ts";
      export { WORKSHOP_FRAME_GUARDS } from "./src/services/protocol.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});

const bundlePath = path.join(os.tmpdir(), "promptforge-workshop-wire-fixtures-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { lifecycle, WorkshopSocket, WORKSHOP_FRAME_GUARDS } = await import(
  pathToFileURL(bundlePath).href
);

const fixture = JSON.parse(
  await readFile(
    path.join(testDir, "..", "..", "..", "workshop", "protocol", "tests", "fixtures", "workshop-frames.json"),
    "utf8",
  ),
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Both suites pin exactly the same case list, so a case added on one side
// fails the other. This list is mirrored by the Rust fixture test.
const CASES = [
  "error",
  "models",
  "select_model",
  "status",
  "switch_profile",
  "workbench",
];
check(
  "the fixture holds exactly the cases both suites pin",
  isDeepStrictEqual(Object.keys(fixture).sort(), CASES),
);

const fakeSockets = [];
class FakeWebSocket {
  static OPEN = 1;
  readyState = 0;
  sent = [];
  onopen = null;
  onclose = null;
  onerror = null;
  onmessage = null;
  constructor(url) {
    this.url = url;
    fakeSockets.push(this);
  }
  send(data) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
  }
  // Test-side controls, not part of the WebSocket surface.
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  message(frame) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}
globalThis.WebSocket = FakeWebSocket;

const warnings = [];
console.warn = (...args) => warnings.push(args.join(" "));

// --- Guards: the inbound frames /ws handles ------------------------------

const INBOUND = ["models", "status", "workbench"];
const fixtureTypes = new Set(Object.values(fixture).map((frame) => frame.type));
check(
  "every inbound type the workshop socket handles appears in the fixture",
  Object.keys(WORKSHOP_FRAME_GUARDS).every((type) => fixtureTypes.has(type)),
);
check(
  "the workshop socket guards exactly the inbound cases",
  isDeepStrictEqual(Object.keys(WORKSHOP_FRAME_GUARDS).sort(), INBOUND),
);
for (const type of INBOUND) {
  check(
    `the ${type} fixture frame passes its guard`,
    WORKSHOP_FRAME_GUARDS[type](fixture[type]) === true,
  );
}

await assertNoLeaks(lifecycle, async () => {
  // --- Server-to-client: each fixture frame routes through unchanged ------

  const socket = new WorkshopSocket("ws://fake/ws");
  const statuses = [];
  const models = [];
  const workbenches = [];
  socket.onStatus((frame) => statuses.push(frame));
  socket.onModels((list) => models.push(list));
  socket.onWorkbench((frame) => workbenches.push(frame));
  socket.ready();
  socket.connect();
  const wire = fakeSockets[0];
  wire.open();

  wire.message(fixture.status);
  wire.message(fixture.models);
  wire.message(fixture.workbench);

  check(
    "the status fixture frame delivers verbatim",
    isDeepStrictEqual(statuses, [fixture.status]),
  );
  check(
    "the models fixture frame delivers its catalog verbatim",
    isDeepStrictEqual(models, [fixture.models.models]),
  );
  check(
    "the workbench fixture frame delivers verbatim",
    isDeepStrictEqual(workbenches, [fixture.workbench]),
  );

  // The error frame is a refusal answered to an inbound event; the
  // workshop socket has no emitter for it, so it must not surface as a
  // push.
  wire.message(fixture.error);
  check(
    "an error fixture frame does not surface as a push",
    statuses.length === 1 && models.length === 1 && workbenches.length === 1,
  );
  check("an error fixture frame on /ws warns about nothing", warnings.length === 0);

  // Every required field of every inbound frame: a copy without it fails
  // its guard and is dropped, with one warning naming its type.
  for (const type of INBOUND) {
    for (const field of Object.keys(fixture[type]).filter((key) => key !== "type")) {
      const copy = structuredClone(fixture[type]);
      delete copy[field];
      warnings.length = 0;
      wire.message(copy);
      check(
        `a ${type} frame missing ${field} fails its guard`,
        WORKSHOP_FRAME_GUARDS[type](copy) === false,
      );
      check(
        `a ${type} frame missing ${field} is dropped without a handler call`,
        statuses.length === 1 && models.length === 1 && workbenches.length === 1,
      );
      check(
        `a ${type} frame missing ${field} warns once, naming ${type}`,
        warnings.length === 1 && warnings[0].includes(type),
      );
    }
  }

  // --- Client-to-server: each send matches its fixture entry --------------

  socket.selectModel(fixture.select_model.model);
  socket.switchProfile(fixture.switch_profile.name);
  check(
    "select_model and switch_profile sends match their fixture entries",
    isDeepStrictEqual(wire.sent, [fixture.select_model, fixture.switch_profile]),
  );
  socket.dispose();
});

if (failures.length > 0) {
  console.error(`workshop-wire-fixtures: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("workshop-wire-fixtures: all assertions passed");
process.exit(0);
