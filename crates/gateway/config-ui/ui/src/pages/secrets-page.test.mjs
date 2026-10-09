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
} from "../test-support.mjs";

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

/** Boots the desk with the env and sheet stubbed and lands on Secrets. */
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

/** Opens Secrets and returns the Hugging Face API Key section's pieces. */
async function openHf(env = envFixture(), wrap = () => undefined) {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), env });
  wrap(stub);
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();
  const card = root.querySelector(".hf-card");
  return {
    dom,
    root,
    stub,
    card,
    input: card.querySelector("input.env-value"),
    state: card.querySelector(".secret-state"),
  };
}

/** Focuses the key field and types into it the way a keystroke does. */
function typeKey(dom, input, value) {
  input.focus();
  input.value = value;
  input.dispatchEvent(new dom.window.Event("input"));
}

test("the Hugging Face section is an API Key row with the Enter API key placeholder and no Verify button", async () => {
  const { root, card, input } = await openHf();
  assert.equal(card.querySelector("h3.section-heading").textContent, "Hugging Face API Key");
  const label = card.querySelector(".hf-row label.env-key");
  assert.equal(label.textContent, "API Key");
  assert.equal(label.htmlFor, input.id, "the visible label names the key field");
  assert.equal(input.type, "password");
  assert.equal(input.placeholder, "Enter API key");
  assert.equal(
    [...root.querySelectorAll("button")].some((button) => /verify/i.test(button.textContent)),
    false,
    "no Verify button anywhere on the page",
  );
});

test("Secret saved shows once a key exists and appears after the first save", async () => {
  const withKey = await openHf();
  assert.equal(withKey.state.textContent, "Secret saved");
  assert.equal(withKey.state.hidden, false);

  const env = envFixture();
  delete env.boot.vars.HF_TOKEN;
  const { dom, stub, input, state } = await openHf(env);
  assert.equal(state.hidden, true, "no key yet, so nothing claims it is saved");
  typeKey(dom, input, "hf-first");
  input.blur();
  await settle();
  assert.equal(stub.state.envPuts.length, 1);
  assert.equal(state.textContent, "Secret saved");
  assert.equal(state.hidden, false);
});

test("the API key saves on blur only when it was edited", async () => {
  const { dom, stub, input } = await openHf();
  input.focus();
  input.blur();
  await settle();
  assert.equal(stub.state.envPuts.length, 0, "a blur with no edit saves nothing");

  typeKey(dom, input, "hf-edited");
  assert.equal(stub.state.envPuts.length, 0, "typing alone does not save");
  input.blur();
  await settle();
  assert.equal(stub.state.envPuts.length, 1, "the blur after an edit saves once");
  assert.deepEqual(stub.state.envPuts[0], {
    scope: "global",
    vars: {
      GATEWAY_KEY: "boot-master-key",
      HF_TOKEN: "hf-edited",
      OPENAI_KEY: "sk-fixture",
    },
  });

  input.focus();
  input.blur();
  await settle();
  assert.equal(stub.state.envPuts.length, 1, "the saved key is no longer an edit");
});

test("Enter and Escape blur the API key field, and the blur saves an edit", async () => {
  const { dom, stub, input } = await openHf();
  for (const [key, expectedPuts] of [
    ["Enter", 1],
    ["Escape", 2],
  ]) {
    typeKey(dom, input, `hf-after-${key}`);
    input.dispatchEvent(new dom.window.KeyboardEvent("keydown", { key, bubbles: true }));
    await settle();
    assert.notEqual(dom.window.document.activeElement, input, `${key} blurs the field`);
    assert.equal(stub.state.envPuts.length, expectedPuts, `the ${key} blur saves the edit`);
  }
});

test("a failed key save raises an error toast and the next blur retries", async () => {
  const env = envFixture();
  delete env.boot.vars.HF_TOKEN;
  let failures = 1;
  const { dom, root, stub, input, state } = await openHf(env, (stub) => {
    const fetchFn = stub.fetchFn;
    stub.fetchFn = async (url, init = {}) => {
      if (String(url).includes("/admin/env") && init.method === "PUT" && failures > 0) {
        failures -= 1;
        return jsonResponse({ error: { message: "disk full" } }, 500);
      }
      return fetchFn(url, init);
    };
  });
  typeKey(dom, input, "hf-retry");
  input.blur();
  await settle();
  assert.equal(stub.state.envPuts.length, 0, "the refused save never reached the stub's record");
  assert.equal(state.hidden, true, "a refused save does not claim Secret saved");
  assert.match(root.querySelector(".toast-error, .toast")?.textContent ?? "", /disk full/);

  input.focus();
  input.blur();
  await settle();
  assert.equal(stub.state.envPuts.length, 1, "the key is still an edit, so the next blur retries");
  assert.equal(state.textContent, "Secret saved");
});
