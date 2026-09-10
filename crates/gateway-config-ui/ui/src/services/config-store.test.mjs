// Pins the pure pending-STT predicate the Apply path snapshots before
// its post-apply refresh: the [stt] tuning section, the [[stt_model]]
// catalog, the STT membership of the effective active profile, and the
// active-profile pointer decide whether a restart is needed; unrelated
// edits never do.
import assert from "node:assert/strict";
import test from "node:test";

import { loadApp, modelsFixture } from "../harness.mjs";

const { pendingSttChange } = await loadApp();

/** A (running, pending) pair from one fixture plus a pending mutation. */
function pair(mutate = null) {
  const running = modelsFixture();
  const pending = structuredClone(running);
  mutate?.(pending);
  return [running, pending];
}

test("identical views mean no speech change", () => {
  const [running, pending] = pair();
  assert.equal(pendingSttChange(running, pending), false);
});

test("unrelated model, settings, and endpoint edits are not speech changes", () => {
  let [running, pending] = pair((doc) => {
    doc.local_model[1].description = "a chat edit";
  });
  assert.equal(pendingSttChange(running, pending), false, "a chat model edit");

  [running, pending] = pair((doc) => {
    doc.server.bind = "0.0.0.0:9999";
  });
  assert.equal(pendingSttChange(running, pending), false, "a [server] edit");

  [running, pending] = pair((doc) => {
    doc.endpoint.push({
      id: "new-ep",
      protocol: "openai",
      base_url: "https://api.example.test/v1",
      api_key: "***",
    });
  });
  assert.equal(pendingSttChange(running, pending), false, "an endpoint addition");
});

test("adding, editing, or removing the [stt] tuning section qualifies", () => {
  let [running, pending] = pair((doc) => {
    doc.stt = { window_seconds: 15, interval_ms: 500, vocabulary: [] };
  });
  assert.equal(pendingSttChange(running, pending), true, "adding the section");

  [running, pending] = pair((doc) => {
    doc.stt = { window_seconds: 8, interval_ms: 250, vocabulary: ["WG21"] };
  });
  running.stt = { window_seconds: 15, interval_ms: 500, vocabulary: [] };
  assert.equal(pendingSttChange(running, pending), true, "editing a tuning field");

  [running, pending] = pair();
  running.stt = { window_seconds: 15, interval_ms: 500, vocabulary: [] };
  assert.equal(pendingSttChange(running, pending), true, "removing the section");
});

test("STT catalog additions, removals, and edits qualify", () => {
  let [running, pending] = pair((doc) => {
    doc.stt_model.push({
      name: "whisper-small-en",
      role: "final",
      source: "models/ggml-small.en.bin",
      sha256: null,
      vram_gb: 2,
    });
  });
  assert.equal(pendingSttChange(running, pending), true, "adding a model");

  [running, pending] = pair((doc) => {
    doc.stt_model = [];
  });
  assert.equal(pendingSttChange(running, pending), true, "removing a model");

  [running, pending] = pair((doc) => {
    doc.stt_model[0].source = "models/ggml-large-v3.bin";
  });
  assert.equal(pendingSttChange(running, pending), true, "editing a model field");
});

test("reordering the STT catalog alone is not a speech change", () => {
  const extra = {
    name: "whisper-small-en",
    role: "final",
    source: "models/ggml-small.en.bin",
    sha256: null,
    vram_gb: 2,
  };
  const [running, pending] = pair((doc) => {
    doc.stt_model.push(structuredClone(extra));
  });
  running.stt_model.push(structuredClone(extra));
  pending.stt_model.reverse();
  assert.equal(pendingSttChange(running, pending), false, "same entries, different order");
});

test("only the effective active profile's STT membership matters", () => {
  // The inactive travel profile gains the STT model: no boot effect.
  let [running, pending] = pair((doc) => {
    doc.profile.find((p) => p.name === "travel").models.push("whisper-base-en");
  });
  assert.equal(pendingSttChange(running, pending), false, "an inactive profile's membership");

  // The active default profile loses it: the boot selection changes.
  [running, pending] = pair((doc) => {
    const profile = doc.profile.find((p) => p.name === "default");
    profile.models = profile.models.filter((name) => name !== "whisper-base-en");
  });
  assert.equal(pendingSttChange(running, pending), true, "the active profile loses STT");

  // The active profile gains a chat model: no speech effect.
  [running, pending] = pair((doc) => {
    doc.profile.find((p) => p.name === "default").models.push("llama-leaf");
  });
  assert.equal(pendingSttChange(running, pending), false, "the active profile gains chat");
});

test("the active-profile pointer qualifies only when STT membership differs", () => {
  // travel selects no STT model: switching to it changes the boot selection.
  let [running, pending] = pair((doc) => {
    doc.active_profile = "travel";
  });
  assert.equal(pendingSttChange(running, pending), true, "a pointer to an STT-free profile");

  // Give travel the same STT membership: the pointer move changes nothing.
  [running, pending] = pair((doc) => {
    doc.active_profile = "travel";
  });
  running.profile.find((p) => p.name === "travel").models.push("whisper-base-en");
  pending.profile.find((p) => p.name === "travel").models.push("whisper-base-en");
  assert.equal(pendingSttChange(running, pending), false, "a pointer to equal STT membership");
});
