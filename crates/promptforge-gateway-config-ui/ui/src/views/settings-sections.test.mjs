// Pins the Settings view's editable panels: the Gateway card's
// single-config save (untouched secrets ride through as "***", a typed
// key leaves the DOM after the save, the restart and new-key notes),
// the Workshop Enable flow with STT/tape subsections, dominion cards
// (kind-dependent vram_gb, used-by chips, dependent-naming delete, the
// focused draft), endpoint cards (Change-reveal secret, remote-only
// dominion options), the Storage save, the Tools Enable flow, the
// restart banner after apply, that a store notification
// on another route cannot hand the pane back to Settings, and that a
// profile save preserves every staged global section.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

function fixtureStub(extra = {}) {
  return gatewayStub({
    key: "k",
    config: modelsFixture(),
    models: ["qwen-common"],
    ...extra,
  });
}

/** The recorded PUT bodies for one route suffix. */
function putBodies(stub, suffix) {
  return stub.calls
    .filter((call) => call.url.endsWith(suffix) && call.init.method === "PUT")
    .map((call) => JSON.parse(call.init.body));
}

function changeValue(dom, input, value) {
  input.value = value;
  input.dispatchEvent(new dom.window.Event("change"));
}

function chooseDropdown(scope, key, value) {
  const row = scope.querySelector(`.field-row[data-key='${key}']`);
  row.querySelector(".select").click();
  const option = [...row.querySelectorAll(".menu-item")].find(
    (item) => item.dataset.value === value,
  );
  assert.ok(option, `dropdown ${key} offers ${value}`);
  option.click();
}

test("a Gateway save PUTs the global shadow with the untouched api_key as ***", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  assert.match(
    root.querySelector(".restart-note").textContent,
    /Restart required/,
    "the restart-bound card carries the restart note",
  );
  assert.match(
    root.querySelector(".configui-url").textContent,
    /^http:\/\/127\.0\.0\.1:8081\/config\/$/,
    "the Config UI card derives its URL from the gateway bind",
  );
  assert.ok(
    root.querySelector(".secret-change"),
    "the stored api_key renders masked with a Change button",
  );

  const bind = root.querySelector(".field-row[data-key='bind'] input");
  changeValue(dom, bind, "0.0.0.0:9999");
  await settle();
  root.querySelector(".card-save").click();
  await settle();

  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies.length, 1, "the save PUTs /admin/config once");
  assert.equal(bodies[0].server.bind, "0.0.0.0:9999");
  assert.equal(bodies[0].server.api_key, "***", "the untouched secret rides through redacted");
});

test("changing the gateway api_key warns about the new key and saves it", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  assert.equal(root.querySelector(".secret-input"), null, "no input before Change is clicked");
  root.querySelector(".secret-change").click();
  await settle();
  const input = root.querySelector(".secret-input");
  assert.ok(input, "Change reveals the password input");
  changeValue(dom, input, "new-master-key");
  await settle();
  assert.match(
    root.querySelector(".new-key-warning").textContent,
    /After restart, you will need to enter the new API key/,
  );
  root.querySelector(".card-save").click();
  await settle();
  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies[0].server.api_key, "new-master-key");
});

test("a saved new api_key leaves the DOM and the masked readout returns", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  root.querySelector(".secret-change").click();
  await settle();
  changeValue(dom, root.querySelector(".secret-input"), "new-master-key");
  await settle();
  root.querySelector(".card-save").click();
  await settle();

  assert.equal(root.querySelector(".secret-input"), null, "the password input is gone");
  assert.ok(root.querySelector(".secret-change"), "the masked Change readout returns");
  assert.ok(
    [...root.querySelectorAll("input")].every((input) => input.value !== "new-master-key"),
    "no input in the DOM still holds the typed key",
  );
});

test("Workshop exposes STT capture tuning without legacy model paths", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/workshop");
  await settle();
  root.querySelector(".workshop-enable").click();
  await settle();

  const bind = root.querySelector(".field-row[data-key='bind'] input");
  assert.equal(bind.value, "127.0.0.1:7910", "the section arrives with its defaults");
  assert.ok(
    root.querySelector(".field-row[data-key='open_browser'] .switch"),
    "open_browser renders as a toggle",
  );

  root.querySelector(".add-stt").click();
  await settle();
  assert.match(root.querySelector(".workshop-stt").textContent, /STT capture tuning/);
  assert.equal(
    root.querySelector(".field-row[data-key='stt.window_seconds'] input").value,
    "15",
    "the STT capture defaults mirror the config crate",
  );
  assert.ok(root.querySelector(".field-row[data-key='stt.vocabulary'] .chip-input, .field-row[data-key='stt.vocabulary'] input"));
  assert.equal(root.querySelector("[data-key='stt.interim_model']"), null);
  assert.equal(root.querySelector("[data-key='stt.final_source']"), null);

  root.querySelector(".add-tape").click();
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='tape.path'] input").value,
    "tape.jsonl",
  );

  root.querySelector(".card-save").click();
  await settle();
  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies.length, 1);
  assert.equal(bodies[0].workshop.bind, "127.0.0.1:7910");
  assert.equal(bodies[0].workshop.stt.window_seconds, 15);
  assert.equal(bodies[0].workshop.tape.path, "tape.jsonl");
  assert.equal(
    bodies[0].server.bind,
    "127.0.0.1:8081",
    "a Workshop save still carries the global [server] section",
  );
});

test("a local dominion shows vram_gb, and switching kind to remote hides it", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/dominions");
  await settle();
  const card = root.querySelector(".entry-card[data-entry='dominion:gpu0']");
  assert.ok(card, "the fixture dominion renders as a card");
  card.querySelector(".entry-toggle").click();
  assert.ok(
    card.querySelector(".field-row[data-key='vram_gb']"),
    "a local-kind dominion reveals the vram_gb field",
  );

  chooseDropdown(card, "kind", "remote");
  await settle();
  const rerendered = root.querySelector(".entry-card[data-entry='dominion:gpu0']");
  assert.equal(
    rerendered.querySelector(".field-row[data-key='vram_gb']"),
    null,
    "switching to remote hides vram_gb",
  );
});

test("used-by chips count dependents and a delete warns naming them", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/dominions");
  await settle();
  const chip = root.querySelector(".entry-card[data-entry='dominion:gpu0'] .used-by-chip");
  assert.equal(chip.textContent, "used by 2", "gpu0 is used by chat and STT models");
  assert.match(chip.title, /local model 'qwen-common'/);
  assert.match(chip.title, /STT model 'whisper-base-en'/);

  root.querySelector(".entry-card[data-entry='dominion:gpu0'] .entry-delete").click();
  await settle();
  const body = dom.window.document.querySelector("#confirm-body");
  assert.match(
    body.textContent,
    /used by local model 'qwen-common'/,
    "the delete confirm names the dependents",
  );
  dom.window.document.querySelector(".modal .button-danger").click();
  await settle();
  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies.length, 1, "the confirmed delete saves the shadow");
  assert.deepEqual(bodies[0].dominion, [], "the dominion entry is gone from the payload");
});

test("Add Dominion opens an expanded draft card with the id input focused", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/dominions");
  await settle();
  root.querySelector(".entry-add").click();
  const card = root.querySelector(".entry-card[data-entry='dominion-draft:0']");
  assert.ok(card, "the draft card renders");
  assert.ok(card.querySelector(".draft-badge"), "the draft is marked unsaved");
  const idInput = card.querySelector(".field-row[data-key='id'] input");
  assert.equal(dom.window.document.activeElement, idInput, "the empty id input has focus");
});

test("the endpoint dominion dropdown offers only remote-kind dominions", async () => {
  const config = modelsFixture();
  config.dominion.push({
    id: "pool-r",
    kind: "remote",
    max_queue: 100,
    policy: "queue",
    fair_scheduling: true,
  });
  const stub = fixtureStub({ config });
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/endpoints");
  await settle();
  const card = root.querySelector(".entry-card[data-entry='endpoint:openai']");
  card.querySelector(".entry-toggle").click();
  const trigger = card.querySelector(".field-row[data-key='dominion'] .select");
  trigger.click();
  const options = [...card.querySelectorAll(".field-row[data-key='dominion'] .menu-item")].map(
    (option) => option.textContent,
  );
  assert.deepEqual(
    options,
    ["None", "pool-r"],
    "the local-kind gpu0 is excluded; None and the remote pool remain",
  );
});

test("a Storage save PUTs the global shadow with the new cache_dir", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/system");
  await settle();
  const input = root.querySelector(".field-row[data-key='cache_dir'] input");
  changeValue(dom, input, "D:/pf-cache");
  await settle();
  root.querySelector(".card-save").click();
  await settle();

  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies.length, 1, "the save PUTs /admin/config once");
  assert.equal(bodies[0].local.cache_dir, "D:/pf-cache");
  assert.equal(bodies[0].dominion.length, 1, "the keyed arrays ride through the payload");
});

test("every settings save carries the complete single-file configuration", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/system");
  await settle();
  changeValue(dom, root.querySelector(".field-row[data-key='cache_dir'] input"), "D:/pf-cache");
  await settle();
  root.querySelector(".card-save").click();
  await settle();

  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies.length, 1);
  assert.equal(bodies[0].server.bind, "127.0.0.1:8081");
  assert.equal(bodies[0].profile.length, 2);
});

test("one global section save does not erase another staged section", async () => {
  // Stage a [server] edit, then save [local]. The pending view must
  // keep both edits, and a later Workshop save must carry them.
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  changeValue(dom, root.querySelector(".field-row[data-key='bind'] input"), "0.0.0.0:9999");
  await settle();
  root.querySelector(".card-save").click();
  await settle();

  navigate(dom, "#/settings/system");
  await settle();
  changeValue(dom, root.querySelector(".field-row[data-key='cache_dir'] input"), "D:/pf-cache");
  await settle();
  root.querySelector(".card-save").click();
  await settle();

  assert.equal(
    stub.state.pending.server.bind,
    "0.0.0.0:9999",
    "the staged [server] edit survives the [local] save",
  );
  navigate(dom, "#/settings/gateway");
  await settle();
  assert.equal(
    root.querySelector(".field-row[data-key='bind'] input").value,
    "0.0.0.0:9999",
    "the Gateway card still renders the staged bind",
  );

  navigate(dom, "#/settings/workshop");
  await settle();
  root.querySelector(".workshop-enable").click();
  await settle();
  root.querySelector(".card-save").click();
  await settle();
  const globalBodies = putBodies(stub, "/admin/config");
  assert.equal(
    globalBodies[globalBodies.length - 1].server.bind,
    "0.0.0.0:9999",
    "a Workshop-only save carries the staged [server] edit, not a stale copy",
  );
});

test("an endpoint's api_key stays *** through a save and Change reveals the input", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/endpoints");
  await settle();
  const card = root.querySelector(".entry-card[data-entry='endpoint:openai']");
  assert.match(card.querySelector(".used-by-chip").title, /model 'gpt-remote'/);
  card.querySelector(".entry-toggle").click();

  const protocol = card.querySelector(".field-row[data-key='protocol'] .select");
  assert.equal(protocol.value, "openai");
  assert.ok(protocol.disabled, "the protocol dropdown is locked");
  assert.equal(
    card.querySelector(".secret-input"),
    null,
    "the api_key input is hidden until Change is clicked",
  );
  assert.ok(card.querySelector(".secret-change"));

  const baseUrl = card.querySelector(".field-row[data-key='base_url'] input");
  changeValue(dom, baseUrl, "https://proxy.example/v1");
  await settle();
  root
    .querySelector(".entry-card[data-entry='endpoint:openai'] .card-save")
    .click();
  await settle();
  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies[0].endpoint[0].base_url, "https://proxy.example/v1");
  assert.equal(
    bodies[0].endpoint[0].api_key,
    "***",
    "the untouched endpoint secret rides through redacted",
  );

  const reopened = root.querySelector(".entry-card[data-entry='endpoint:openai']");
  reopened.querySelector(".secret-change").click();
  await settle();
  assert.ok(
    root.querySelector(".entry-card[data-entry='endpoint:openai'] .secret-input"),
    "Change reveals the endpoint password input",
  );
});

test("Tools Enable renders the web_search card with the spec's fields", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/tools");
  await settle();
  assert.match(root.querySelector(".settings-panel").textContent, /Web search not configured/);
  root.querySelector(".tools-enable").click();
  await settle();

  const provider = root.querySelector(".field-row[data-key='provider'] .select");
  assert.equal(provider.value, "brave");
  assert.ok(provider.disabled, "the provider dropdown is locked to Brave");
  assert.equal(
    root.querySelector(".field-row[data-key='base_url'] input").placeholder,
    "https://api.search.brave.com/res/v1",
  );
  assert.equal(root.querySelector(".field-row[data-key='default_count'] input").placeholder, "10");
  assert.equal(root.querySelector(".field-row[data-key='max_count'] input").placeholder, "20");
  assert.equal(root.querySelector(".field-row[data-key='max_per_host'] input").placeholder, "2");
  assert.equal(
    root
      .querySelector(".field-row[data-key='strip_tracking'] .switch")
      .getAttribute("aria-checked"),
    "true",
    "strip_tracking defaults on",
  );

  root.querySelector(".card-save").click();
  await settle();
  const bodies = putBodies(stub, "/admin/config");
  assert.equal(bodies[0].tools.web_search.provider, "brave");
});

test("a store notification on the Models route cannot hand the pane back to Settings", async () => {
  const stub = fixtureStub({
    dirty: { dirty: true, pending_files: ["gateway.toml"], changed_sections: [] },
  });
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  navigate(dom, "#/local");
  await settle();
  // Apply refreshes the store and notifies every view; the settings view
  // subscribed after the models view, so an unguarded re-render would
  // paint Settings into the shared main pane last and win it.
  root.querySelector(".apply-button").click();
  await settle();

  assert.ok(root.querySelector(".models-split"), "the Models view still owns the pane");
  assert.equal(root.querySelector(".settings-panel"), null, "the Settings panel stays unmounted");
});

test("the restart banner clears when config generation advances", async () => {
  const stub = fixtureStub({
    dirty: { dirty: true, pending_files: ["gateway.toml"], changed_sections: ["server"] },
    applyOutcome: { applied: ["gateway.toml"], reloaded: false, restart_required: true },
  });
  const { root } = await bootApp({ key: "k", stub });

  const banner = root.querySelector(".banner-restart");
  assert.ok(banner.hidden, "the restart banner starts hidden");
  root.querySelector(".apply-button").click();
  await settle();
  assert.ok(!banner.hidden, "a restart-required apply reveals the banner");
  assert.match(banner.textContent, /Restart the gateway/);
  stub.state.configGeneration = "generation-2";
  await new Promise((resolve) => setTimeout(resolve, 1_050));
  await settle();
  assert.ok(banner.hidden, "a new gateway generation dismisses the stale restart banner");
});

test("blurring a chip input commits the pending text as a chip", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/workshop");
  await settle();
  root.querySelector(".workshop-enable").click();
  await settle();
  root.querySelector(".add-stt").click();
  await settle();

  const chipInput = root.querySelector(".field-row[data-key='stt.vocabulary'] .chip-input input");
  assert.ok(chipInput, "the vocabulary chip input renders");
  chipInput.value = "GGUF";
  chipInput.dispatchEvent(new dom.window.Event("blur"));
  await settle();

  const chips = [...root.querySelectorAll(".field-row[data-key='stt.vocabulary'] .pill")];
  assert.ok(
    chips.some((chip) => chip.textContent.includes("GGUF")),
    "blurring commits the typed value as a chip",
  );
});

test("blurring a chip input with an empty value does not add a chip", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/workshop");
  await settle();
  root.querySelector(".workshop-enable").click();
  await settle();
  root.querySelector(".add-stt").click();
  await settle();

  const chipInput = root.querySelector(".field-row[data-key='stt.vocabulary'] .chip-input input");
  chipInput.value = "";
  chipInput.dispatchEvent(new dom.window.Event("blur"));
  await settle();

  const chips = [...root.querySelectorAll(".field-row[data-key='stt.vocabulary'] .pill")];
  assert.equal(chips.length, 0, "blurring an empty input adds no chip");
});

test("the secret field toggle switches the password input type", async () => {
  const stub = fixtureStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  navigate(dom, "#/settings/gateway");
  await settle();
  root.querySelector(".secret-change").click();
  await settle();

  const input = root.querySelector(".secret-input");
  assert.equal(input.type, "password", "the input starts as a password field");
  const toggle = root.querySelector(".secret-toggle");
  assert.ok(toggle, "the Eye/EyeOff toggle button renders");
  assert.equal(toggle.getAttribute("aria-label"), "Show");

  toggle.click();
  assert.equal(input.type, "text", "clicking the toggle reveals the secret");
  assert.equal(toggle.getAttribute("aria-label"), "Hide");
  assert.equal(toggle.getAttribute("aria-pressed"), "true");

  toggle.click();
  assert.equal(input.type, "password", "clicking again re-hides the secret");
  assert.equal(toggle.getAttribute("aria-label"), "Show");
  assert.equal(toggle.getAttribute("aria-pressed"), "false");
});

test("Restore recommended models creates or resets the pinned STT pair", async () => {
  const config = modelsFixture();
  config.stt_model[0].source = "models/stale.bin";
  const stub = fixtureStub({ config });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/settings/workshop");
  await settle();
  root.querySelector(".workshop-enable").click();
  await settle();

  root.querySelector(".restore-recommended").click();
  await settle();
  const names = stub.state.pending.stt_model.map((model) => model.name);
  assert.deepEqual(names, ["whisper-base-en", "whisper-small-en"]);
  assert.match(stub.state.pending.stt_model[0].source, /ggml-base\.en\.bin$/);
  assert.equal(stub.state.pending.stt_model[0].sha256.length, 64);
  assert.equal(stub.state.pending.stt_model[1].role, "final");
});
