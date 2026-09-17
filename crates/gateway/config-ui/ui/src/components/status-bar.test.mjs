// Pins the bottom status bar: the idle LED strip maps each endpoint's
// ready/provisioning flags to its LED state beside the model/VRAM
// summary; an active queue command swaps the shared shell's slot to the
// progress bar with the command label in the text region, the pending
// count with per-entry cancel buttons, and a cancel button that calls
// POST /admin/queue/cancel; and panel mode mounts no bar at
// all (the workshop owns status display there).
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, settle } from "../harness.mjs";

const ENDPOINTS = [
  { path: "/v1/chat/completions", name: "Chat completions", ready: true, provisioning: false },
  { path: "/v1/embeddings", name: "Embeddings", ready: false, provisioning: true },
  { path: "/v1/rerank", name: "Rerank", ready: false, provisioning: false },
];

test("the idle bar maps each endpoint to its LED state plus the model summary", async () => {
  const stub = gatewayStub({
    key: "k",
    config: modelsFixture(),
    models: ["a", "b"],
    endpoints: ENDPOINTS,
    vramGb: 4.1,
  });
  const { root } = await bootApp({ key: "k", stub });

  const bar = root.querySelector(".status-bar");
  assert.ok(bar, "the status bar mounts in standalone mode");
  const leds = [...bar.querySelectorAll(".status-led")];
  assert.equal(leds.length, 3, "one LED per endpoint");
  assert.equal(leds[0].dataset.state, "ready", "a served endpoint is green");
  assert.equal(leds[1].dataset.state, "provisioning", "a provisioning endpoint is amber");
  assert.equal(leds[2].dataset.state, "unconfigured", "an unconfigured endpoint is gray");
  assert.match(leds[0].title, /Chat completions \(\/v1\/chat\/completions\): ready/);
  assert.equal(
    bar.querySelector(".status-bar-summary").textContent,
    "2 models, 4.1 GB",
    "the summary carries the model count and declared VRAM",
  );
  assert.equal(bar.querySelector(".status-bar__progress").hidden, true, "no progress bar idle");
});

test("the idle bar omits the VRAM total when nothing declares any", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), models: ["a"] });
  const { root } = await bootApp({ key: "k", stub });
  assert.equal(
    root.querySelector(".status-bar-summary").textContent,
    "1 model",
    "a single model is singular and a zero VRAM total is omitted",
  );
});

test("an active command swaps the slot to the progress bar, and cancel calls the route", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const stub = gatewayStub({ key: "k", config: modelsFixture(), endpoints: ENDPOINTS });
  const { root } = await bootApp({ key: "k", stub });
  const indicators = root.querySelector(".status-bar__indicators");
  const progress = root.querySelector(".status-bar__progress");
  assert.equal(indicators.hidden, false, "the LED strip shows while the queue is idle");

  stub.state.queue = {
    active: { name: "load-profile: main", fraction: 0.34, started_at: 1_700_000_000 },
    pending: [{ name: "provision-model: extra", queued_at: 1_700_000_001 }],
  };
  t.mock.timers.tick(2000);
  await settle();

  assert.equal(indicators.hidden, true, "the LED strip hides while a command runs");
  assert.equal(progress.hidden, false, "the progress bar takes the slot");
  assert.equal(
    root.querySelector(".status-bar__text").textContent,
    "load-profile: main (34%)",
    "the text carries the command name and rounded percent",
  );
  assert.equal(progress.value, 34, "the bar reads the rounded percent");
  assert.equal(progress.max, 100);
  assert.equal(
    root.querySelector(".status-bar-pending").textContent,
    "1 queued",
    "the pending count shows while commands wait",
  );

  const pendingCancel = root.querySelector(".status-bar-pending-cancel");
  assert.ok(pendingCancel, "each queued command gets its own cancel button");
  assert.match(pendingCancel.textContent, /provision-model: extra/);
  pendingCancel.click();
  await settle();
  assert.deepEqual(
    stub.state.cancelPendingCalls,
    [{ index: 0 }],
    "the pending cancel button fired the cancel-pending route with the entry's index",
  );

  root.querySelector(".status-bar-cancel").click();
  await settle();
  assert.equal(stub.state.cancelActiveCalls, 1, "the cancel button fired the cancel route");

  // The command settled: the next poll swaps back to the LED strip.
  stub.state.queue = { active: null, pending: [] };
  t.mock.timers.tick(2000);
  await settle();
  assert.equal(indicators.hidden, false, "the LED strip returns once the queue drains");
  assert.equal(progress.hidden, true);
});

test("panel mode mounts no status bar", async () => {
  const { root } = await bootApp({ url: "http://127.0.0.1:8081/config/?mode=panel" });
  assert.equal(
    root.querySelector(".status-bar"),
    null,
    "the workshop owns status display in panel mode",
  );
});
