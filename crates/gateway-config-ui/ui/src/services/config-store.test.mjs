// Pins the store's discovered-model staging against the persisted
// selection: a Discover download joins the profile the pending envelope
// names (not the running one), and with no selection it joins the
// catalog alone while the toast says so.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  gatewayStub,
  hfModelFixture,
  hfSearchFixture,
  modelsFixture,
  navigate,
  settle,
  systemFixture,
} from "../harness.mjs";

const REPO = "unsloth/Qwen3-Test-8B-GGUF";

/** Boots to Discover, opens the fixture repo, and stages its first quant. */
async function stageFirstQuant(selected) {
  const config = modelsFixture();
  const stub = gatewayStub({
    key: "k",
    config,
    pending: structuredClone(config),
    selected,
    hfSearch: hfSearchFixture(),
    hfModels: { [REPO]: hfModelFixture() },
    system: systemFixture(),
  });
  const booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/discover");
  await settle();
  booted.root.querySelector(".result-row").click();
  await settle();
  booted.root.querySelector(".quant-download").click();
  await settle();
  const staged = stub.state.pending.local_model.find((entry) =>
    String(entry.source).startsWith("https://huggingface.co/"),
  );
  assert.ok(staged, "the quant joined the pending local catalog");
  const members = (name) =>
    stub.state.pending.profile.find((profile) => profile.name === name).models;
  return { ...booted, stub, staged, members };
}

test("a discovered model joins the selected profile, not the running one", async () => {
  const { root, staged, members } = await stageFirstQuant("travel");
  assert.ok(members("travel").includes(staged.name), "the persisted selection gains the model");
  assert.ok(
    !members("default").includes(staged.name),
    "the running profile is not the target when the selection differs",
  );
  assert.ok(
    [...root.querySelectorAll(".toast-success")].some((toast) =>
      toast.textContent.includes(staged.name),
    ),
    "the success toast names the staged model",
  );
});

test("with no selected profile the model joins the catalog alone and the toast says so", async () => {
  const { root, staged, members } = await stageFirstQuant(null);
  assert.ok(!members("default").includes(staged.name), "no profile gains the model");
  assert.ok(!members("travel").includes(staged.name));
  assert.ok(
    [...root.querySelectorAll(".toast")].some((toast) =>
      /no profile is selected/i.test(toast.textContent),
    ),
    "the toast says no profile is selected",
  );
});

test("the persisted selection never rides a config PUT", async () => {
  const { stub } = await stageFirstQuant("travel");
  const puts = stub.calls.filter(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  assert.ok(puts.length > 0, "staging wrote the pending config");
  assert.ok(
    puts.every((call) => !("active_profile" in JSON.parse(call.init.body))),
    "the envelope's active_profile is stripped before the payload is built",
  );
});
