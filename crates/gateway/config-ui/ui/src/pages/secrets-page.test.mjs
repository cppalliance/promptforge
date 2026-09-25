import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  cloudSheetFixture,
  envFixture,
  gatewayStub,
  jsonResponse,
  modelsFixture,
  navigate,
  settle,
} from "../harness.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * The cloud sheet fixture plus a Bedrock slice (two key variables and a
 * defaulted region) and a failed-status Legacy slice whose one key
 * variable already exists in the env fixture's gateway.env.
 */
function secretsSheetFixture() {
  const sheet = cloudSheetFixture();
  sheet.providers.bedrock = {
    display_name: "Bedrock",
    tier: "prime",
    status: "ok",
    fetched_at: "2026-09-14T13:00:00Z",
    openai_base_url: null,
    env_vars: [
      { name: "AWS_ACCESS_KEY_ID", role: "key", default: null },
      { name: "AWS_SECRET_ACCESS_KEY", role: "key", default: null },
      { name: "AWS_REGION", role: "config", default: "us-east-1" },
    ],
    models: [],
  };
  sheet.providers.legacy = {
    display_name: "Legacy",
    tier: "niche",
    status: "failed",
    fetched_at: null,
    openai_base_url: null,
    env_vars: [{ name: "OPENAI_KEY", role: "key", default: null }],
    models: [],
  };
  return sheet;
}

/** Boots the shell with the env and sheet stubbed and lands on Secrets. */
async function openSecrets(stubOptions = {}) {
  const stub = gatewayStub({
    key: "k",
    config: modelsFixture(),
    env: envFixture(),
    ...stubOptions,
  });
  const { dom, root } = await bootApp({ key: "k", stub, options: { sheetPollMs: 10 } });
  navigate(dom, "#/secrets");
  await settle();
  return { dom, root, stub };
}

/** Selects a provider in the Secrets view's dropdown: opens the trigger, clicks the row. */
function chooseProvider(dom, root, name) {
  root.querySelector(".env-provider-select .select").click();
  root.querySelector(`.env-provider-select .menu-item[data-value='${name}']`).click();
}

test("Secrets edits the single global environment file", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), env: envFixture() });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();

  const sections = root.querySelectorAll(".env-section");
  assert.equal(sections.length, 1);
  assert.match(sections[0].textContent, /Global environment \(gateway\.env\)/);
  assert.ok(sections[0].querySelector(".hf-row"), "HF_TOKEN belongs to the global file");

  const value = sections[0].querySelector(".env-row[data-key='OPENAI_KEY'] .env-value");
  value.value = "sk-edited";
  value.dispatchEvent(new dom.window.Event("input"));
  sections[0].querySelector(".env-save").click();
  await settle();
  assert.deepEqual(stub.state.envPuts[0], {
    scope: "global",
    vars: {
      GATEWAY_KEY: "boot-master-key",
      HF_TOKEN: "hf-fixture-token",
      OPENAI_KEY: "sk-edited",
    },
  });
  const call = stub.calls.find(
    (entry) => entry.url.includes("/admin/env") && entry.init.method === "PUT",
  );
  assert.equal(call.url.includes("scope="), false, "global is the default and only write scope");
});

test("the global HF token connection probe uses the gateway proxy", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), env: envFixture() });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();
  root.querySelector(".hf-test").click();
  await settle();
  assert.equal(root.querySelector(".hf-status").textContent, "Valid");
  assert.ok(stub.calls.some((call) => call.url.includes("/admin/hf/search")));
});

test("leaving Secrets aborts its environment load without repainting the route", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), env: envFixture() });
  const fetch = stub.fetchFn;
  let aborted = false;
  stub.fetchFn = async (input, init = {}) => {
    if (String(input).includes("/admin/env")) {
      return new Promise((_resolve, reject) => {
        init.signal.addEventListener("abort", () => {
          aborted = true;
          reject(new DOMException("aborted", "AbortError"));
        });
      });
    }
    return fetch(input, init);
  };
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();
  navigate(dom, "#/local");
  await settle();

  assert.equal(aborted, true);
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Local");
});

test("selecting a single-key provider fills the new-variable NAME", async () => {
  const { dom, root } = await openSecrets({ cloudModels: secretsSheetFixture() });
  chooseProvider(dom, root, "anthropic");
  assert.equal(root.querySelector(".env-add-key").value, "ANTHROPIC_API_KEY");
  assert.equal(
    root.querySelector(".env-row[data-key='ANTHROPIC_API_KEY']"),
    null,
    "a single-entry provider appends no further rows",
  );
  const trigger = root.querySelector(".env-provider-select .select");
  assert.equal(trigger.id, "env-add-provider-global", "the hidden label points at the trigger");
  assert.equal(trigger.value, "", "the picker resets to the placeholder after a selection");
  assert.equal(trigger.textContent, "Add from provider…");
});

test("selecting Bedrock appends its secret and region rows with the default prefilled", async () => {
  const { dom, root } = await openSecrets({ cloudModels: secretsSheetFixture() });
  chooseProvider(dom, root, "bedrock");
  assert.equal(
    root.querySelector(".env-add-key").value,
    "AWS_ACCESS_KEY_ID",
    "the first key-role variable fills NAME",
  );
  const secret = root.querySelector(".env-row[data-key='AWS_SECRET_ACCESS_KEY'] .env-value");
  assert.ok(secret, "the second key variable becomes a row");
  assert.equal(secret.value, "", "key values stay empty");
  const region = root.querySelector(".env-row[data-key='AWS_REGION'] .env-value");
  assert.ok(region, "the config variable becomes a row");
  assert.equal(region.value, "us-east-1", "the config default prefills");
});

test("keyless and already-present providers are greyed in the dropdown", async () => {
  const { dom, root } = await openSecrets({ cloudModels: secretsSheetFixture() });
  const option = (name) =>
    root.querySelector(`.env-provider-select .menu-item[data-value='${name}']`);
  assert.equal(option("acme").disabled, true, "a keyless provider greys");
  assert.equal(option("acme").getAttribute("aria-disabled"), "true");
  assert.equal(
    option("legacy").disabled,
    true,
    "a provider whose key is already in gateway.env greys",
  );
  assert.equal(option("legacy").getAttribute("aria-disabled"), "true");
  assert.equal(option("anthropic").disabled, false);
  assert.equal(option("anthropic").hasAttribute("aria-disabled"), false);
  assert.equal(option("bedrock").disabled, false);
  chooseProvider(dom, root, "acme");
  assert.equal(root.querySelector(".env-add-key").value, "", "a greyed row fills nothing");
  assert.equal(root.querySelector(".env-row[data-key='OPENAI_KEY']").isConnected, true);
  chooseProvider(dom, root, "legacy");
  assert.equal(
    root.querySelectorAll(".env-row[data-key='OPENAI_KEY']").length,
    1,
    "a greyed present-key provider appends no duplicate row",
  );
});

test("every provider slice appears in the dropdown regardless of status", async () => {
  const { root } = await openSecrets({ cloudModels: secretsSheetFixture() });
  const control = root.querySelector(".env-provider-select");
  assert.equal(control.querySelector("select"), null, "no native select remains");
  const rows = [...control.querySelectorAll(".menu-item")];
  assert.equal(rows[0].dataset.value, "", "the placeholder row leads");
  assert.equal(rows[0].textContent, "Add from provider…");
  const values = rows.map((row) => row.dataset.value);
  for (const name of ["anthropic", "openai", "bedrock", "acme", "deepgram", "legacy"]) {
    assert.ok(values.includes(name), `${name} is listed`);
  }
  const groups = [...control.querySelectorAll(".menu-group-label")].map((g) => g.textContent);
  assert.deepEqual(groups, ["Prime", "Subprime", "Niche"], "one group header per non-empty tier");
  const children = [...control.querySelector(".menu").children];
  assert.equal(
    children.indexOf(control.querySelector(".menu-group-label")),
    1,
    "the first tier header sits right after the placeholder row",
  );
  for (const header of control.querySelectorAll(".menu-group-label")) {
    assert.equal(header.getAttribute("role"), "presentation");
    assert.equal(header.tabIndex, -1, "headers are not focusable");
  }
});

test("remounting Secrets clears a stale provider NAME fill", async () => {
  const { dom, root } = await openSecrets({ cloudModels: secretsSheetFixture() });
  chooseProvider(dom, root, "anthropic");
  assert.equal(root.querySelector(".env-add-key").value, "ANTHROPIC_API_KEY");
  navigate(dom, "#/local");
  await settle();
  navigate(dom, "#/secrets");
  await settle();
  assert.equal(
    root.querySelector(".env-add-key").value,
    "",
    "the NAME fill from the previous mount is gone",
  );
});

test("the provider dropdown appears in place when the sheet lands", async () => {
  let release = false;
  const { root } = await openSecrets({
    onCloudModels: () =>
      release
        ? jsonResponse(secretsSheetFixture())
        : jsonResponse({ error: { message: "downloading", code: "cloud_models_loading" } }, 503),
  });
  assert.equal(root.querySelector(".env-provider-select"), null, "no dropdown before the sheet");
  assert.ok(root.querySelector(".env-add-key"), "the free-text row works as today");
  release = true;
  await sleep(200);
  assert.ok(root.querySelector(".env-provider-select"), "the dropdown appears in place");
});
