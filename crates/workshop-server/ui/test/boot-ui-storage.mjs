// Boot test for the UI-state preload in the composition root (src/main.ts):
// the app fetches GET /user/state and GET /workspace/file/state before it
// resolves any service (the first getService call, recorded by the boot
// fixture's registry seam as a RESOLVE entry in the fetch log), and a
// server that never answers still lets boot complete on defaults after the
// preload timeout with one warning per bucket. Stores are registry
// services, so the first resolution of any token bounds the first store
// construction from above.
//
// bootWorkbench exits the process, so each scenario boots in its own child
// process; run without arguments this file drives both and fails if either
// does. Run: node test/boot-ui-storage.mjs (after `npm run build`).
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { bootWorkbench } from "./helpers/boot.mjs";

const USER_STATE = "/user/state";
const WORKSPACE_STATE = "/workspace/file/state";
// main.ts preloads with a 3000 ms timeout; the hanging scenario allows the
// timer some slack either side.
const PRELOAD_TIMEOUT_MS = 3000;

// Counts the adapter's own console warnings across the whole boot; the
// bundle writes to the Node global console, which jsdom does not replace.
const uiStorageWarnings = [];
const realWarn = console.warn;
console.warn = (...args) => {
  const text = args.map(String).join(" ");
  if (text.startsWith("ui-storage:")) {
    uiStorageWarnings.push(text);
  } else {
    realWarn(...args);
  }
};

const isStateGet = (entry) =>
  entry.method === "GET" && (entry.url === USER_STATE || entry.url === WORKSPACE_STATE);
const stateGets = (fetchLog) => fetchLog.filter(isStateGet);
// The first service resolution of the boot; `url` carries the token id.
const firstResolve = (fetchLog) => fetchLog.find((entry) => entry.method === "RESOLVE");

// Both state GETs recorded once each, both before the first getService.
function checkOrdering(fetchLog, failures) {
  const gets = stateGets(fetchLog);
  const urls = gets.map((entry) => entry.url).sort();
  if (urls.join(",") !== [USER_STATE, WORKSPACE_STATE].join(",")) {
    failures.push(
      `boot must GET each state bucket exactly once, saw ${JSON.stringify(gets.map((e) => e.url))}`,
    );
  }
  const resolveIndex = fetchLog.findIndex((entry) => entry.method === "RESOLVE");
  if (resolveIndex === -1) {
    failures.push("boot never resolved a service; the registry seam is not recording");
    return;
  }
  const lastStateGet = fetchLog.reduce((last, entry, index) => (isStateGet(entry) ? index : last), -1);
  if (lastStateGet > resolveIndex) {
    failures.push(
      `both state GETs must precede the first getService (${fetchLog[resolveIndex].url}); log: ${JSON.stringify(fetchLog.map((e) => `${e.method} ${e.url}`))}`,
    );
  }
}

const scenarios = {
  // Both buckets answer: the two GETs are the first entries of the boot
  // log, ahead of every service resolution, and nothing warns.
  resolve: () =>
    bootWorkbench(
      "boot preloads both UI-state buckets before any store resolves",
      async ({ fetchLog, failures }) => {
        checkOrdering(fetchLog, failures);
        const firstTwo = fetchLog.slice(0, 2).map((entry) => `${entry.method} ${entry.url}`).sort();
        if (firstTwo.join(",") !== `GET ${USER_STATE},GET ${WORKSPACE_STATE}`) {
          failures.push(`the state GETs must be the first two fetches, saw ${JSON.stringify(firstTwo)}`);
        }
        if (uiStorageWarnings.length !== 0) {
          failures.push(`a clean preload must not warn, saw ${JSON.stringify(uiStorageWarnings)}`);
        }
      },
      { uiState: { user: {}, workspace: {} } },
    ),

  // Both buckets hang: boot still completes (bootWorkbench itself fails
  // when the panels never mount), the first service resolves only after
  // the preload timeout rather than early, and each bucket warns exactly
  // once.
  hang: () =>
    bootWorkbench(
      "boot completes on defaults when both UI-state buckets hang",
      async ({ fetchLog, failures }) => {
        checkOrdering(fetchLog, failures);
        const gets = stateGets(fetchLog);
        const resolve = firstResolve(fetchLog);
        if (gets.length === 2 && resolve) {
          const waited = resolve.at - Math.max(...gets.map((entry) => entry.at));
          if (waited < PRELOAD_TIMEOUT_MS - 200) {
            failures.push(
              `the first service resolved ${waited} ms after the state GETs; boot must wait out the ${PRELOAD_TIMEOUT_MS} ms preload timeout`,
            );
          }
          if (waited > PRELOAD_TIMEOUT_MS + 2000) {
            failures.push(
              `the first service resolved ${waited} ms after the state GETs; boot must not block past the timeout`,
            );
          }
        }
        const perBucket = [USER_STATE, WORKSPACE_STATE].map(
          (route) => uiStorageWarnings.filter((text) => text.includes(`GET ${route}`)).length,
        );
        if (perBucket.join(",") !== "1,1") {
          failures.push(
            `each hanging bucket must warn exactly once, saw ${JSON.stringify(uiStorageWarnings)}`,
          );
        }
      },
      { uiState: { user: "hang", workspace: "hang" } },
    ),
};

const scenarioFlag = process.argv.find((arg) => arg.startsWith("--scenario="));
if (scenarioFlag) {
  const scenario = scenarios[scenarioFlag.slice("--scenario=".length)];
  if (!scenario) {
    console.error(`unknown scenario ${scenarioFlag}; known: ${Object.keys(scenarios).join(", ")}`);
    process.exit(1);
  }
  await scenario();
} else {
  // Driver: one child per scenario, output forwarded, any failure fails
  // the file. Sequential so the two boots' timing assertions never share a
  // CPU with each other.
  const self = fileURLToPath(import.meta.url);
  let failed = 0;
  for (const name of Object.keys(scenarios)) {
    const code = await new Promise((resolve) => {
      const child = spawn(process.execPath, [self, `--scenario=${name}`], { stdio: "inherit" });
      child.on("exit", (exitCode) => resolve(exitCode ?? 1));
      child.on("error", () => resolve(1));
    });
    if (code !== 0) failed += 1;
  }
  process.exit(failed === 0 ? 0 : 1);
}
