import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture, settle } from "../harness.mjs";

test("the switcher lists pending profiles and checks the active one", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  const { root } = await bootApp({ key: "k", stub });
  root.querySelector(".profile-switcher button").click();
  await settle();
  assert.deepEqual(
    [...root.querySelectorAll("[role='menuitemradio']")].map((row) => [
      row.textContent,
      row.getAttribute("aria-checked"),
    ]),
    [
      ["default", "true"],
      ["travel", "false"],
    ],
  );
});

test("selecting in the switcher stages active_profile without an immediate switch", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  const { root } = await bootApp({ key: "k", stub });
  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "travel")
    .click();
  await settle();
  assert.equal(stub.state.pending.active_profile, "travel");
  assert.equal(stub.state.active, "default");
  assert.equal(
    stub.calls.some((call) => call.url.endsWith("/admin/switch-profile")),
    false,
  );
  assert.match(root.querySelector(".profile-switcher > button").title, /Apply/);
});
