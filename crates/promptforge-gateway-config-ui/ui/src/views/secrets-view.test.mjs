// Pins the Secrets view: the two env sections (profile open and first,
// boot collapsed and editable), masked rows with per-row reveal,
// add/edit/delete flowing into the PUT /admin/env payload with the
// applied-on notes, the HF Token card's Test Connection statuses, and
// the ${VAR} cross-reference annotations the GET /admin/env reply
// carries (computed server-side from the raw pre-interpolation chain).
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  envFixture,
  gatewayStub,
  modelsFixture,
  navigate,
  settle,
} from "../harness.mjs";

/** A stub with both env files (the fixture references ${OPENAI_KEY}). */
function secretsStub(extra = {}) {
  return gatewayStub({
    key: "k",
    config: modelsFixture(),
    env: envFixture(),
    ...extra,
  });
}

/** Fires the input event after typing into a field. */
function typeValue(dom, input, value) {
  input.value = value;
  input.dispatchEvent(new dom.window.Event("input"));
}

test("the sections render profile-first with boot collapsed, rows masked, reveal toggling", async () => {
  const stub = secretsStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();

  const sections = root.querySelectorAll(".env-section");
  assert.equal(sections.length, 2, "both env files render");
  assert.equal(sections[0].dataset.scope, "profile", "the profile section comes first");
  assert.match(sections[0].textContent, /Profile environment \(default\.env\)/);
  assert.equal(sections[1].dataset.scope, "boot", "the boot section comes second");
  assert.equal(sections[1].tagName, "DETAILS", "the boot section is collapsible");
  assert.equal(sections[1].open, false, "the boot section starts collapsed");
  assert.match(sections[1].textContent, /Boot environment \(gateway\.env\)/);
  assert.ok(
    sections[1].querySelector(".env-save"),
    "the boot section is editable (PUT /admin/env?scope=boot exists)",
  );

  const row = sections[0].querySelector(".env-row[data-key='OPENAI_KEY']");
  const value = row.querySelector(".env-value");
  assert.equal(value.type, "password", "values are masked by default");
  assert.equal(value.value, "sk-fixture", "the plaintext value is present, just masked");

  const toggle = row.querySelector(".reveal-toggle");
  toggle.click();
  assert.equal(value.type, "text", "the reveal toggle unmasks the row");
  assert.equal(toggle.getAttribute("aria-pressed"), "true");
  toggle.click();
  assert.equal(value.type, "password", "the toggle masks it again");
});

test("add, edit, and delete flow into the PUT payload and the notes render", async () => {
  const stub = secretsStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();

  const profile = root.querySelector(".env-section[data-scope='profile']");
  assert.match(
    profile.querySelector(".env-note").textContent,
    /Applied on restart or profile switch/,
    "the profile note names the apply moments",
  );
  assert.match(
    profile.querySelector(".env-note").textContent,
    /already set in the gateway's process keeps its old value until a full restart/,
    "the note carries the dotenv no-override caveat",
  );

  // Edit the existing variable.
  typeValue(
    dom,
    profile.querySelector(".env-row[data-key='OPENAI_KEY'] .env-value"),
    "sk-edited",
  );
  // Add a new one.
  typeValue(dom, profile.querySelector(".env-add-key"), "NEW_VAR");
  typeValue(dom, profile.querySelector(".env-add-value"), "fresh");
  profile.querySelector(".env-add").click();
  await settle();
  profile.querySelector(".env-save").click();
  await settle();

  assert.deepEqual(
    stub.state.envPuts[0],
    {
      scope: "profile",
      vars: { HF_TOKEN: "hf-fixture-token", OPENAI_KEY: "sk-edited", NEW_VAR: "fresh" },
    },
    "the save PUTs the edited and added variables to the profile scope",
  );

  // Delete the variable and save again.
  profile.querySelector(".env-row[data-key='NEW_VAR'] .env-delete").click();
  await settle();
  profile.querySelector(".env-save").click();
  await settle();
  assert.deepEqual(
    stub.state.envPuts[1].vars,
    { HF_TOKEN: "hf-fixture-token", OPENAI_KEY: "sk-edited" },
    "a deleted row leaves the next payload",
  );

  // The boot section saves to its own scope.
  const boot = root.querySelector(".env-section[data-scope='boot']");
  assert.match(
    boot.querySelector(".env-note").textContent,
    /after a gateway restart/,
    "the boot note says restart",
  );
  boot.querySelector(".env-save").click();
  await settle();
  assert.deepEqual(
    stub.state.envPuts[2],
    { scope: "boot", vars: { GATEWAY_KEY: "boot-master-key" } },
    "the boot save PUTs with scope=boot",
  );
});

test("HF Test Connection reports Valid, Invalid, and Not set", async () => {
  // Valid: the stub's HF proxy answers 200.
  let stub = secretsStub();
  let booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/secrets");
  await settle();
  booted.root.querySelector(".hf-test").click();
  await settle();
  assert.equal(booted.root.querySelector(".hf-status").textContent, "Valid");
  assert.ok(
    stub.calls.some((c) => c.url.includes("/admin/hf/search")),
    "the probe rides the gateway HF proxy",
  );

  // Invalid: the hub refuses with the pass-through 401.
  stub = secretsStub({ hfAuth401: true });
  booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/secrets");
  await settle();
  booted.root.querySelector(".hf-test").click();
  await settle();
  assert.equal(booted.root.querySelector(".hf-status").textContent, "Invalid");

  // Not set: no HF_TOKEN in the profile env file.
  const env = envFixture();
  delete env.profile.vars.HF_TOKEN;
  stub = secretsStub({ env });
  booted = await bootApp({ key: "k", stub });
  navigate(booted.dom, "#/secrets");
  await settle();
  const before = stub.calls.length;
  booted.root.querySelector(".hf-test").click();
  await settle();
  assert.equal(booted.root.querySelector(".hf-status").textContent, "Not set");
  assert.equal(stub.calls.length, before, "an unset token probes nothing");
});

test("a server-reported ${VAR} reference annotates its row", async () => {
  const stub = secretsStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();

  const row = root.querySelector(".env-row[data-key='OPENAI_KEY']");
  assert.equal(
    row.querySelector(".env-used-by").textContent,
    "used by: endpoint openai api_key",
    "the annotation names the referencing field by entry identity",
  );
  const unreferenced = root.querySelector(".env-row[data-key='GATEWAY_KEY']");
  assert.equal(
    unreferenced.querySelector(".env-used-by"),
    null,
    "an unreferenced variable carries no annotation",
  );
});

test("typing into an absent HF token joins the rows and enables its delete", async () => {
  const env = envFixture();
  delete env.profile.vars.HF_TOKEN;
  const stub = secretsStub({ env });
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/secrets");
  await settle();

  const hfRow = root.querySelector(".hf-row");
  const remove = hfRow.querySelector(".env-delete");
  assert.equal(remove.disabled, true, "a placeholder token has nothing to delete");
  typeValue(dom, hfRow.querySelector(".env-value"), "hf-new");
  assert.equal(remove.disabled, false, "the typed token is deletable at once");

  root.querySelector(".env-section[data-scope='profile'] .env-save").click();
  await settle();
  assert.equal(
    stub.state.envPuts[0].vars.HF_TOKEN,
    "hf-new",
    "the typed token joins the saved payload",
  );
});
