// Pins the tab bar's profile switcher against the switch route: a pick
// posts `POST /admin/switch-profile`, never stages `active_profile` in a
// config PUT, raises the restart banner when the gateway reports
// `restart_required`, and lists "No profile" first, checked when the
// pending envelope reports no persisted selection.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, settle } from "../harness.mjs";

/** `[text, aria-checked]` of every row in the open switcher menu. */
function rows(root) {
  return [...root.querySelectorAll("[role='menuitemradio']")].map((row) => [
    row.textContent,
    row.getAttribute("aria-checked"),
  ]);
}

/** Opens the switcher and clicks the row whose text is `name`. */
async function pick(root, name) {
  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === name)
    .click();
  await settle();
}

/** Every PUT /admin/config body the stub received. */
function putBodies(stub) {
  return stub.calls
    .filter((call) => call.url.endsWith("/admin/config") && call.init.method === "PUT")
    .map((call) => JSON.parse(call.init.body));
}

test("the switcher lists No profile first, then every profile, with the selection checked", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  const { root } = await bootApp({ key: "k", stub });
  root.querySelector(".profile-switcher button").click();
  await settle();
  assert.deepEqual(rows(root), [
    ["No profile", "false"],
    ["default", "true"],
    ["travel", "false"],
  ]);
});

test("selecting posts switch-profile, stages nothing, and raises the banner on restart_required", async (t) => {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  // The restart poll clears itself once the generation moves on.
  t.after(async () => {
    stub.state.configGeneration = "generation-2";
    await new Promise((resolve) => setTimeout(resolve, 1_050));
  });
  const { root } = await bootApp({ key: "k", stub });

  await pick(root, "travel");

  assert.deepEqual(stub.state.switchCalls, [{ name: "travel" }], "the pick posts the route");
  assert.equal(stub.state.selected, "travel", "the gateway persisted the selection");
  assert.equal(stub.state.active, "default", "the running profile is untouched");
  assert.ok(
    putBodies(stub).every((body) => !("active_profile" in body)),
    "no config PUT carries active_profile",
  );
  assert.equal(
    root.querySelector(".banner-restart").hidden,
    false,
    "restart_required raises the restart banner",
  );
  const label = root.querySelector(".profile-switcher > button");
  assert.match(label.textContent, /travel/, "the trigger shows the selected profile");
  assert.match(label.title, /restart/i, "the title says a restart runs it");
});

test("re-selecting the running profile reports no restart and leaves the banner hidden", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), selected: "travel" });
  const { root } = await bootApp({ key: "k", stub });

  await pick(root, "default");

  assert.deepEqual(stub.state.switchCalls, [{ name: "default" }]);
  assert.equal(root.querySelector(".banner-restart").hidden, true, "no restart is needed");
  assert.equal(root.querySelector(".profile-switcher > button").title, "");
});

test("No profile is checked when the envelope reports null and sends null when picked", async (t) => {
  const stub = gatewayStub({ key: "k", config: modelsFixture(), selected: null });
  t.after(async () => {
    stub.state.configGeneration = "generation-2";
    await new Promise((resolve) => setTimeout(resolve, 1_050));
  });
  const { root } = await bootApp({ key: "k", stub });
  root.querySelector(".profile-switcher button").click();
  await settle();
  assert.deepEqual(rows(root), [
    ["No profile", "true"],
    ["default", "false"],
    ["travel", "false"],
  ]);
  root.querySelector(".profile-switcher button").click();
  await settle();

  // Move to travel and back to No profile: the null selection rides the wire.
  await pick(root, "travel");
  await pick(root, "No profile");
  assert.deepEqual(
    stub.state.switchCalls,
    [{ name: "travel" }, { name: null }],
    "No profile posts null",
  );
  assert.equal(stub.state.selected, null);
  assert.match(root.querySelector(".profile-switcher > button").textContent, /No profile/);
});

test("a profile name containing a quote selects and leaves the rows enabled", async (t) => {
  const config = modelsFixture();
  const quoted = 'say "hi"';
  config.profile.push({ name: quoted, models: ["llama-leaf"] });
  const stub = gatewayStub({ key: "k", config });
  t.after(async () => {
    stub.state.configGeneration = "generation-2";
    await new Promise((resolve) => setTimeout(resolve, 1_050));
  });
  const { root } = await bootApp({ key: "k", stub });

  await pick(root, quoted);

  assert.deepEqual(stub.state.switchCalls, [{ name: quoted }], "the quoted name rides the wire");
  assert.match(root.querySelector(".profile-switcher > button").textContent, /say "hi"/);
  root.querySelector(".profile-switcher button").click();
  await settle();
  assert.ok(
    [...root.querySelectorAll("[role='menuitemradio']")].every((row) => !row.disabled),
    "the switcher is usable after the pick",
  );
});

test("a refused switch toasts the gateway's message and keeps the old selection", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  const fetch = stub.fetchFn;
  stub.fetchFn = async (input, init = {}) => {
    if (String(input).endsWith("/admin/switch-profile")) {
      return new Response(
        JSON.stringify({ error: { message: "the state file is read-only", type: "server_error" } }),
        { status: 500, headers: { "content-type": "application/json" } },
      );
    }
    return fetch(input, init);
  };
  const { root } = await bootApp({ key: "k", stub });

  await pick(root, "travel");

  const toast = root.ownerDocument.querySelector(".toast-error");
  assert.equal(toast?.textContent, "the state file is read-only");
  assert.equal(stub.state.selected, "default", "the refusal changed nothing");
  assert.match(root.querySelector(".profile-switcher > button").textContent, /default/);
});
