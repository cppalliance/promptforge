import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, navigate, settle } from "./harness.mjs";

test("the tab bar uses the required order and routes Local and Remote details", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  assert.deepEqual(
    [...root.querySelectorAll(".tab")].map((tab) => tab.textContent),
    ["Settings", "Discover", "Local", "Remote", "Profiles", "Secrets"],
  );

  for (const [hash, title] of [
    ["#/settings", "Settings"],
    ["#/discover", "Discover"],
    ["#/local/model-a", "Local"],
    ["#/remote/model-b", "Remote"],
    ["#/profiles", "Profiles"],
    ["#/secrets", "Secrets"],
  ]) {
    navigate(dom, hash);
    await settle();
    assert.equal(root.querySelector("main h1.view-title")?.textContent, title);
    assert.equal(root.querySelector(".tab[aria-current='page']")?.textContent, title);
  }
});

test("unknown, Downloads, and legacy Models routes normalize to Local", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  for (const hash of ["#/unknown", "#/downloads", "#/models", "#/local/%"]) {
    navigate(dom, hash);
    await settle();
    assert.equal(dom.window.location.hash, "#/local");
    assert.equal(root.querySelector("main h1.view-title")?.textContent, "Local");
  }
});

test("booting without a hash lands on Local", async () => {
  const stub = gatewayStub();
  const { root } = await bootApp({ key: "k", stub });
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Local");
});
