// Pins the add-model merge: endpoint creation versus reuse, the
// ${VAR} key indirection from the slice, keyless providers, the
// missing-context rejection, and the sheet-to-config capability field
// mapping.
import assert from "node:assert/strict";
import test from "node:test";

import { cloudSheetFixture, loadApp } from "../harness.mjs";

const app = await loadApp();

const sheet = cloudSheetFixture();
const anthropic = sheet.providers.anthropic;
const fable = anthropic.models[0];

test("a new provider appends its endpoint and the model entry", () => {
  const config = {};
  app.mergeCloudModel(config, "anthropic", anthropic, fable, { name: "Claude Fable 5.1" });
  assert.deepEqual(config.endpoint, [
    {
      id: "anthropic",
      protocol: "openai",
      base_url: "https://api.anthropic.com/v1",
      api_key: "${ANTHROPIC_API_KEY}",
    },
  ]);
  assert.equal(config.model.length, 1);
  const [model] = config.model;
  assert.equal(model.name, "Claude Fable 5.1");
  assert.equal(model.upstream, "claude-fable-5-1", "the upstream is the sheet id");
  assert.deepEqual(model.endpoints, ["anthropic"]);
  assert.equal(model.kind, "chat");
  assert.equal(model.context, 200000);
  assert.equal(model.description, "Claude Fable 5.1", "the description defaults to the display name");
});

test("the capability fields mirror the sheet's field names", () => {
  const config = {};
  app.mergeCloudModel(config, "anthropic", anthropic, fable, { name: "fable" });
  const [model] = config.model;
  assert.equal(model.thinking, "switchable", "manual budget mode is a per-call switch");
  assert.equal(model.adaptive_thinking, true);
  assert.equal(model.images, true);
  assert.equal(model.max_output, 64000);
  assert.deepEqual(model.effort_levels, ["low", "high"]);
  assert.equal(model.default_effort, "low");
});

test("thinking maps supported-only to always and unsupported to never", () => {
  const gpt = sheet.providers.openai.models[0];
  const alwaysConfig = {};
  app.mergeCloudModel(alwaysConfig, "openai", sheet.providers.openai, gpt, { name: "gpt" });
  assert.equal(alwaysConfig.model[0].thinking, "always");
  assert.equal(alwaysConfig.model[0].adaptive_thinking, false);

  const acme = sheet.providers.acme.models[0];
  const neverConfig = {};
  app.mergeCloudModel(neverConfig, "acme", sheet.providers.acme, acme, {
    name: "acme",
    context: 8192,
  });
  assert.equal(neverConfig.model[0].thinking, "never");
});

test("an existing endpoint is reused, not duplicated", () => {
  const config = {
    endpoint: [
      {
        id: "anthropic",
        protocol: "openai",
        base_url: "https://api.anthropic.com/v1",
        api_key: "***",
      },
    ],
  };
  app.mergeCloudModel(config, "anthropic", anthropic, fable, { name: "fable" });
  assert.equal(config.endpoint.length, 1, "no second anthropic endpoint");
  assert.equal(config.endpoint[0].api_key, "***", "the existing endpoint is untouched");
});

test("a keyless provider's endpoint omits api_key", () => {
  const config = {};
  app.mergeCloudModel(config, "acme", sheet.providers.acme, sheet.providers.acme.models[0], {
    name: "acme",
    context: 8192,
  });
  assert.deepEqual(config.endpoint, [
    { id: "acme", protocol: "openai", base_url: "https://api.acme.test/v1" },
  ]);
});

test("the merge rejects a missing context window the operator did not supply", () => {
  const acme = sheet.providers.acme.models[0];
  assert.throws(
    () => app.mergeCloudModel({}, "acme", sheet.providers.acme, acme, { name: "acme" }),
    /context/,
  );
  const config = {};
  app.mergeCloudModel(config, "acme", sheet.providers.acme, acme, { name: "acme", context: 8192 });
  assert.equal(config.model[0].context, 8192, "the operator-supplied context wins");
});

test("a provider without an OpenAI-compatible endpoint cannot merge", () => {
  const deepgram = sheet.providers.deepgram;
  assert.throws(
    () => app.mergeCloudModel({}, "deepgram", deepgram, deepgram.models[0], { name: "nova" }),
    /OpenAI-compatible/,
  );
});
