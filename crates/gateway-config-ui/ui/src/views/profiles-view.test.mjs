import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

async function openProfiles(config = modelsFixture(), extra = {}) {
  const stub = gatewayStub({
    key: "k",
    config,
    models: ["gpt-remote", "qwen-common"],
    ...extra,
  });
  const booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/profiles");
  await settle();
  return { ...booted, stub };
}

/** The profile list row whose name reads `name`. */
function profileRow(root, name) {
  return [...root.querySelectorAll(".profile-list li")].find(
    (row) => row.querySelector(".profile-name")?.textContent === name,
  );
}

/** The pill labels shown on the named profile row. */
function pills(root, name) {
  return [...profileRow(root, name).querySelectorAll(".pill-accent, .pill-stale")].map(
    (pill) => pill.textContent,
  );
}

/** Lets the restart poll clear itself after the test. */
function releaseRestartPoll(t, stub) {
  t.after(async () => {
    stub.state.configGeneration = "generation-2";
    await new Promise((resolve) => setTimeout(resolve, 1_050));
  });
}

function selectOption(root, pane, name) {
  const option = [...root.querySelectorAll(`.shuttle-${pane} [role='option']`)].find(
    (entry) => entry.querySelector(".model-name").textContent === name,
  );
  assert.ok(option, `${name} appears in ${pane}`);
  option.click();
  return option;
}

test("moving a model to Chosen saves the profile in catalog order", async () => {
  const { dom, root, stub } = await openProfiles();
  selectOption(root, "available", "llama-leaf");
  root.querySelector(".shuttle-choose").click();
  await settle();

  const put = stub.calls.find(
    (call) => call.url.endsWith("/admin/config") && call.init.method === "PUT",
  );
  const profile = JSON.parse(put.init.body).profile.find((entry) => entry.name === "default");
  assert.deepEqual(
    profile.models,
    ["qwen-common", "llama-leaf", "whisper-base-en"],
    "Chosen follows the local and STT catalogs instead of click order",
  );
  assert.match(
    root.querySelector("[aria-live='polite']").textContent,
    /moved to Chosen/,
    "the completed move is announced",
  );
  assert.equal(
    dom.window.document.activeElement.querySelector(".model-name")?.textContent,
    "llama-leaf",
    "focus follows the moved option into the destination list",
  );
});

test("Set Active posts switch-profile, stages nothing, and raises the restart banner", async (t) => {
  const { root, stub } = await openProfiles();
  releaseRestartPoll(t, stub);
  [...root.querySelectorAll(".profile-select")]
    .find((button) => button.textContent.includes("travel"))
    .click();
  root.querySelector(".set-active").click();
  await settle();

  assert.deepEqual(stub.state.switchCalls, [{ name: "travel" }], "Set Active posts the route");
  assert.equal(stub.state.selected, "travel", "the gateway persisted the selection");
  assert.equal(stub.state.active, "default", "the running profile is unchanged");
  assert.ok(
    stub.calls
      .filter((call) => call.url.endsWith("/admin/config") && call.init.method === "PUT")
      .every((call) => !("active_profile" in JSON.parse(call.init.body))),
    "no config PUT carries active_profile",
  );
  assert.equal(root.querySelector(".banner-restart").hidden, false, "the banner is raised");
  assert.equal(root.querySelector(".set-active").disabled, true, "the row is now selected");
  assert.deepEqual(pills(root, "default"), ["Active"], "the running profile stays Active");
  assert.deepEqual(pills(root, "travel"), ["Selected"], "the persisted one reads Selected");
});

test("the No profile row sends null through its own Set Active", async (t) => {
  const { root, stub } = await openProfiles();
  releaseRestartPoll(t, stub);
  const row = profileRow(root, "No profile");
  assert.ok(row, "the list opens with a No profile row");
  assert.equal(root.querySelector(".profile-list li"), row, "No profile is the first row");
  row.querySelector(".set-active-none").click();
  await settle();

  assert.deepEqual(stub.state.switchCalls, [{ name: null }], "the row posts a null name");
  assert.equal(stub.state.selected, null);
  assert.equal(root.querySelector(".banner-restart").hidden, false, "leaving a running profile needs a restart");
  assert.deepEqual(pills(root, "No profile"), ["Selected"]);
  assert.deepEqual(pills(root, "default"), ["Active"]);
});

test("the Available list excludes remote models", async () => {
  const { root } = await openProfiles();
  const available = [...root.querySelectorAll(".shuttle-available .model-name")].map(
    (name) => name.textContent,
  );
  assert.deepEqual(available, ["llama-leaf"], "only local and STT models can be chosen");
  assert.ok(!available.includes("gpt-remote"), "the remote catalog never appears");
});

test("Active and Selected pills follow status versus the pending envelope", async () => {
  const { root } = await openProfiles(modelsFixture(), { selected: "travel" });
  assert.deepEqual(pills(root, "default"), ["Active"]);
  assert.deepEqual(pills(root, "travel"), ["Selected"]);
  assert.deepEqual(pills(root, "No profile"), []);
  assert.equal(
    root.querySelector(".profile-summary-title").textContent,
    "travel",
    "the editor opens on the selected profile",
  );
  assert.equal(
    root.querySelector(".profile-delete").disabled,
    true,
    "the selected profile cannot be deleted",
  );

  const same = await openProfiles();
  assert.deepEqual(pills(same.root, "default"), ["Active"], "no Selected pill when they agree");
});

test("a persisted name absent from the profiles shows the Stale pill until a Set Active clears it", async (t) => {
  const { root, stub } = await openProfiles(modelsFixture(), { selected: "retired" });
  releaseRestartPoll(t, stub);
  const stale = root.querySelector(".profile-stale");
  assert.ok(stale, "the stale notice renders");
  assert.match(stale.textContent, /retired/, "the notice names the missing profile");
  assert.ok(stale.querySelector(".pill-stale"), "the notice carries the Stale pill");
  assert.equal(root.querySelector(".set-active").disabled, false, "Set Active is offered");

  root.querySelector(".set-active").click();
  await settle();
  assert.deepEqual(stub.state.switchCalls, [{ name: "default" }]);
  assert.equal(root.querySelector(".profile-stale"), null, "the stale notice clears");
});

test("the shuttle exposes APG listboxes, roving focus, typeahead, counts, and search", async () => {
  const { dom, root } = await openProfiles();
  const list = root.querySelector(".shuttle-chosen [role='listbox']");
  assert.equal(list.getAttribute("aria-multiselectable"), "true");
  const options = [...list.querySelectorAll("[role='option']")];
  assert.equal(options.filter((option) => option.tabIndex === 0).length, 1);

  options[0].focus();
  options[0].dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "End", bubbles: true }),
  );
  assert.equal(
    dom.window.document.activeElement.querySelector(".model-name").textContent,
    "whisper-base-en",
    "End follows the roving tabindex model",
  );
  dom.window.document.activeElement.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "q", bubbles: true }),
  );
  assert.equal(
    dom.window.document.activeElement.querySelector(".model-name").textContent,
    "qwen-common",
    "printable input performs typeahead",
  );
  assert.match(
    root.querySelector(".shuttle-chosen .shuttle-count").textContent,
    /0 selected, 2 of 2 shown/,
  );

  const search = root.querySelector("#profile-chosen-search");
  search.value = "qwen";
  search.dispatchEvent(new dom.window.Event("input"));
  assert.deepEqual(
    [...root.querySelectorAll(".shuttle-chosen .model-name")].map((name) => name.textContent),
    ["qwen-common"],
    "search narrows the pane without changing membership",
  );
  assert.match(
    root.querySelector(".shuttle-chosen .shuttle-count").textContent,
    /1 of 2 shown/,
  );
});

test("New Profile supports Empty and Copy without an include mode", async () => {
  const { dom, root, stub } = await openProfiles();
  root.querySelector(".new-profile").click();
  let dialog = dom.window.document.querySelector(".new-profile-dialog");
  assert.equal(dialog.querySelector("#start-from-include"), null);
  const nameInput = dialog.querySelector("#new-profile-name");
  const submit = dialog.querySelector("button[type='submit']");
  nameInput.focus();
  nameInput.dispatchEvent(
    new dom.window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }),
  );
  assert.equal(
    dom.window.document.activeElement,
    submit,
    "the focus trap skips options in the closed Copy of menu",
  );
  nameInput.value = "blank";
  submit.click();
  await settle();
  assert.deepEqual(
    stub.state.pending.profile.find((profile) => profile.name === "blank").models,
    [],
  );
  assert.equal(root.querySelector(".profile-summary-title").textContent, "blank");
  assert.match(
    dom.window.document.activeElement.textContent,
    /blank/,
    "successful creation selects and focuses the new profile",
  );

  root.querySelector(".new-profile").click();
  dialog = dom.window.document.querySelector(".new-profile-dialog");
  dialog.querySelector("#new-profile-name").value = "copied";
  const copy = dialog.querySelector("#start-from-copy");
  copy.checked = true;
  copy.dispatchEvent(new dom.window.Event("change", { bubbles: true }));
  dialog.querySelector("button[type='submit']").click();
  await settle();
  assert.deepEqual(
    stub.state.pending.profile.find((profile) => profile.name === "copied").models,
    stub.state.pending.profile.find((profile) => profile.name === "default").models,
  );
});

test("VRAM totals follow Chosen and surface unknown contributors", async () => {
  const config = modelsFixture();
  config.dominion[0].vram_gb = 24;
  config.local_model[1].vram_gb = null;
  config.local_model[1].dominion = "gpu0";
  config.profile[0].models.push("llama-leaf");
  const { root } = await openProfiles(config);
  assert.match(root.querySelector(".vram-total").textContent, /^9 GB estimated/);
  assert.match(root.querySelector(".vram-unknown")?.textContent ?? "", /llama-leaf/);
  assert.match(root.querySelector(".vram-budget").textContent, /\+ 1 unknown/);
  assert.match(root.querySelector(".vram-budget").title, /llama-leaf/);

  selectOption(root, "chosen", "qwen-common");
  root.querySelector(".shuttle-unchoose").click();
  await settle();
  assert.match(
    root.querySelector(".vram-total").textContent,
    /^1 GB estimated/,
    "removing a chosen local model updates the sum live",
  );
});

test("VRAM warning starts at 80 percent and over-budget is an error", async () => {
  const config = modelsFixture();
  config.dominion[0].vram_gb = 10;
  config.local_model[0].vram_gb = 8;
  config.stt_model[0].dominion = null;
  const { root } = await openProfiles(config);
  assert.equal(root.querySelector(".vram-budget").dataset.state, "warning");
  assert.ok(root.querySelector(".vram-budget svg"), "the threshold renders a warning icon");

  config.local_model[0].vram_gb = 10.1;
  const over = await openProfiles(config);
  assert.equal(over.root.querySelector(".vram-budget").dataset.state, "over");
  assert.match(
    over.root.querySelector(".vram-info").title,
    /KV cache grows with context length/,
    "the tooltip explains the hidden memory multiplier",
  );
});
