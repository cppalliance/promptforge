// Unit test for the shared reconnect backoff (reconnect-backoff.ts,
// consumed by the Workshop UI's workshop and agent sockets): exponential growth,
// the cap, the reset that a successful open triggers, and the equal jitter
// bounds. Bundles the module with esbuild and drives it against scripted fake
// timers and an injected random source, so the growth, cap, reset, and jitter
// are pinned deterministically without waiting on a real clock: the delay
// argument each schedule hands to setTimeout is captured, and the queued
// callback is fired by hand.
// Run: node --test test/reconnect-backoff.mjs (from crates/workshop/platform).
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";

const platformDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const bundle = await esbuild.build({
  entryPoints: [path.join(platformDir, "reconnect-backoff.ts")],
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

// --- Growth, the cap, and the reset at the jitter's lower bound -------------

// Equal jitter waits half the computed delay plus a random share of the other
// half. A source of 0 pins the lower bound: exactly half of a computed delay
// that still doubles to the cap and resets to the initial delay.
{
  const backoff = new ReconnectBackoff({ initialMs: 1000, maxMs: 3000, random: () => 0 });
  let retries = 0;
  const retry = () => {
    retries += 1;
  };

  backoff.schedule(retry);
  check("the first retry waits half the initial delay at the lower bound", lastDelay() === 500);
  backoff.schedule(retry);
  check(
    "a second schedule while one is waiting stacks nothing",
    pending.filter((entry) => !entry.cleared && !entry.fired).length === 1,
  );

  fireNext();
  check("the first attempt's retry fires", retries === 1);
  backoff.schedule(retry);
  check("the computed delay doubles after a failed attempt", lastDelay() === 1000);

  fireNext();
  backoff.schedule(retry);
  check(
    "the computed delay caps at the maximum instead of doubling past it",
    lastDelay() === 1500,
  );

  fireNext();
  backoff.schedule(retry);
  check("the computed delay stays capped at the maximum", lastDelay() === 1500);

  fireNext();
  backoff.reset();
  backoff.schedule(retry);
  check("reset restores the initial computed delay", lastDelay() === 500);
  fireNext();
}

// --- The jitter's upper bound -----------------------------------------------

// A source just under 1 waits just under the full computed delay, so the
// jitter never pushes a wait past the computed delay or the cap.
{
  const justUnderOne = 0.999;
  const backoff = new ReconnectBackoff({
    initialMs: 1000,
    maxMs: 3000,
    random: () => justUnderOne,
  });
  const justUnder = (delay, full) => delay > full * justUnderOne && delay < full;

  for (const [attempt, full] of [1000, 2000, 3000, 3000].entries()) {
    backoff.schedule(() => {});
    check(
      `attempt ${attempt + 1} waits just under its full ${full}ms delay at the upper bound`,
      justUnder(lastDelay(), full),
    );
    fireNext();
  }

  backoff.reset();
  backoff.schedule(() => {});
  check(
    "reset restores just under the full initial delay at the upper bound",
    justUnder(lastDelay(), 1000),
  );
  fireNext();
}

// --- The defaults -----------------------------------------------------------

{
  const realRandom = Math.random;
  Math.random = () => 0.5;
  const backoff = new ReconnectBackoff();
  backoff.schedule(() => {});
  Math.random = realRandom;
  check(
    "the default jitter source is Math.random over a one-second initial delay",
    lastDelay() === 750,
  );
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
