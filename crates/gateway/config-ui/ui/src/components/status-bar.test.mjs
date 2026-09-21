// Pins the bottom status bar: the idle LED strip maps each endpoint's
// ready/provisioning flags to its LED state beside the model/VRAM
// summary; a busy Progress snapshot shows the shared shell's barberpole
// beside the still-visible LEDs with the activity text in the text
// region; an active queue command adds the pending count with per-entry
// cancel buttons and a cancel button that calls POST /admin/queue/cancel;
// and panel mode mounts no bar at all (the workshop owns status display
// there).
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
    "the summary shows the model count and declared VRAM",
  );
  assert.equal(bar.querySelector(".status-bar__barberpole").hidden, true, "no barberpole idle");
  assert.equal(bar.querySelector("progress"), null, "no <progress> element remains");
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

test("an active command shows the barberpole beside the LEDs, and cancel calls the route", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const stub = gatewayStub({ key: "k", config: modelsFixture(), endpoints: ENDPOINTS });
  const { root } = await bootApp({ key: "k", stub });
  const indicators = root.querySelector(".status-bar__indicators");
  const barberpole = root.querySelector(".status-bar__barberpole");
  assert.equal(indicators.hidden, false, "the LED strip shows while the queue is idle");
  assert.equal(barberpole.hidden, true, "the barberpole hides while the queue is idle");

  stub.state.progress = { busy: true, text: "Downloading qwen 34%" };
  stub.state.queue = {
    active: { name: "load-profile: main", started_at: 1_700_000_000 },
    pending: [{ name: "provision-model: extra", queued_at: 1_700_000_001 }],
  };
  t.mock.timers.tick(2000);
  await settle();

  assert.equal(indicators.hidden, false, "the LED strip stays visible while a command runs");
  assert.equal(barberpole.hidden, false, "the barberpole shows while the snapshot is busy");
  assert.equal(
    root.querySelector(".status-bar__text").textContent,
    "Downloading qwen 34%",
    "the text is the snapshot's activity text, not the command name",
  );
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

  // The command settled: the next poll hides the barberpole.
  stub.state.progress = { busy: false, text: "" };
  stub.state.queue = { active: null, pending: [] };
  t.mock.timers.tick(2000);
  await settle();
  assert.equal(indicators.hidden, false, "the LED strip is still visible once the queue drains");
  assert.equal(barberpole.hidden, true, "the barberpole hides once the snapshot goes idle");
  assert.equal(root.querySelector(".status-bar__text").textContent, "", "the text clears");
});

test("a busy snapshot with no queue command still shows the barberpole and text", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const stub = gatewayStub({ key: "k", config: modelsFixture(), models: ["a"], endpoints: ENDPOINTS });
  const { root } = await bootApp({ key: "k", stub });

  // Startup provisioning and cache downloads report through the hub
  // without a queue command: the snapshot alone drives the busy state.
  stub.state.progress = { busy: true, text: "Downloading model.bin 12%" };
  t.mock.timers.tick(2000);
  await settle();

  assert.equal(root.querySelector(".status-bar__barberpole").hidden, false);
  assert.equal(root.querySelector(".status-bar__text").textContent, "Downloading model.bin 12%");
  assert.equal(
    root.querySelector(".status-bar-summary").hidden,
    false,
    "with no queue command the model summary stays in the extras region",
  );
  assert.equal(root.querySelector(".status-bar-queue").hidden, true, "no cancel controls");
});

test("panel mode mounts no status bar", async () => {
  const { root } = await bootApp({ url: "http://127.0.0.1:8081/config/?mode=panel" });
  assert.equal(
    root.querySelector(".status-bar"),
    null,
    "the workshop owns status display in panel mode",
  );
});
