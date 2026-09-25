// Unit test for the shared reconnect backoff (src/services/reconnect-backoff.ts,
// consumed by workshop-socket.ts and agent-socket.ts): exponential growth,
// the cap, and the reset that a successful open triggers. Bundles the module
// with esbuild and drives it against scripted fake timers, so the growth,
// cap, and reset are pinned deterministically without waiting on a real
// clock: the delay argument each schedule hands to setTimeout is captured,
// and the queued callback is fired by hand.
// Run: node --test test/reconnect-backoff.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `export { ReconnectBackoff } from "./src/services/reconnect-backoff.ts";`,
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

const bundlePath = path.join(os.tmpdir(), "promptforge-reconnect-backoff-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { ReconnectBackoff } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Scripted timers: capture the delay and callback without waiting, so the
// growth, cap, and reset are asserted on the exact delay values the module
// hands to setTimeout.
const pending = [];
let nextId = 1;
globalThis.setTimeout = (fn, delay) => {
  const id = nextId++;
  pending.push({ id, fn, delay, cleared: false, fired: false });
  return id;
};
globalThis.clearTimeout = (id) => {
  const timer = pending.find((entry) => entry.id === id);
  if (timer !== undefined) timer.cleared = true;
};

/** The delay of the most recently scheduled, still-pending attempt. */
function lastDelay() {
  const live = pending.filter((entry) => !entry.cleared && !entry.fired);
  return live.length === 0 ? undefined : live[live.length - 1].delay;
}

/** Fires the oldest pending attempt, as the real timer would. */
function fireNext() {
  const timer = pending.find((entry) => !entry.cleared && !entry.fired);
  if (timer === undefined) throw new Error("no pending timer to fire");
  timer.fired = true;
  timer.fn();
}

// --- Growth, the cap, and the reset -----------------------------------------

{
  const backoff = new ReconnectBackoff({ initialMs: 1000, maxMs: 3000 });
  let retries = 0;
  const retry = () => {
    retries += 1;
  };

  backoff.schedule(retry);
  check("the first retry waits the initial delay", lastDelay() === 1000);
  backoff.schedule(retry);
  check(
    "a second schedule while one is waiting stacks nothing",
    pending.filter((entry) => !entry.cleared && !entry.fired).length === 1,
  );

  fireNext();
  check("the first attempt's retry fires", retries === 1);
  backoff.schedule(retry);
  check("the delay doubles after a failed attempt", lastDelay() === 2000);

  fireNext();
  backoff.schedule(retry);
  check("the delay caps at the maximum instead of doubling past it", lastDelay() === 3000);

  fireNext();
  backoff.schedule(retry);
  check("the delay stays capped at the maximum", lastDelay() === 3000);

  fireNext();
  backoff.reset();
  backoff.schedule(retry);
  check("reset restores the initial delay", lastDelay() === 1000);
  fireNext();
}

// --- The defaults -----------------------------------------------------------

{
  const backoff = new ReconnectBackoff();
  backoff.schedule(() => {});
  check("the default initial delay is one second", lastDelay() === 1000);
  backoff.cancel();
  check(
    "cancel clears the pending timer",
    pending.filter((entry) => !entry.cleared && !entry.fired).length === 0,
  );
}

if (failures.length > 0) {
  console.error(`reconnect-backoff: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("reconnect-backoff: all assertions passed");
process.exit(0);
