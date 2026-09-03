import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

async function openProfiles(config = modelsFixture()) {
  const stub = gatewayStub({ key: "k", config, models: ["gpt-remote", "qwen-common"] });
  const booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/profiles");
  await settle();
  return { ...booted, stub };
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
    ["gpt-remote", "qwen-common", "llama-leaf", "whisper-base-en"],
    "Chosen follows the global catalog instead of click order",
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

test("Set Active stages the pointer and switches only when Apply runs", async () => {
  const { root, stub } = await openProfiles();
  [...root.querySelectorAll(".profile-select")]
    .find((button) => button.textContent.includes("travel"))
    .click();
  root.querySelector(".set-active").click();
  await settle();

  assert.equal(stub.state.pending.active_profile, "travel");
  assert.equal(stub.state.active, "default", "staging leaves the running profile unchanged");
  assert.equal(
    stub.calls.some((call) => call.url.endsWith("/admin/switch-profile")),
    false,
    "the UI never invokes the immediate-switch route",
  );
  root.querySelector(".apply-button").click();
  await settle();
  assert.equal(stub.state.active, "travel", "Apply commits the staged profile pointer");
  assert.equal(
    root.querySelector(".profile-switcher > button").title,
    "",
    "the switcher clears its pending state after Apply",
  );
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
    new dom.window.KeyboardEvent("keydown", { key: "g", bubbles: true }),
  );
  assert.equal(
    dom.window.document.activeElement.querySelector(".model-name").textContent,
    "gpt-remote",
    "printable input performs typeahead",
  );
  assert.match(
    root.querySelector(".shuttle-chosen .shuttle-count").textContent,
    /0 selected, 3 of 3 shown/,
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
    /1 of 3 shown/,
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
