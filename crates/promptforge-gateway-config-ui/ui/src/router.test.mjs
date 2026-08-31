// Pins the hash router: every route mounts its stub view into <main>,
// unknown hashes fall back to #/models, and the tab bar's aria-current
// (the accent underline) follows the active route.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, navigate, settle } from "./harness.mjs";

/**
 * The tab that must carry aria-current after navigating, per hash, and
 * whether the destination is still a stub view (stubs render the shared
 * empty state; live views own their content).
 */
const ROUTES = [
  ["#/models", "Models", false],
  ["#/models/qwen3-8b", "Models", false],
  ["#/discover", "Discover", false],
  ["#/profiles", "Profiles", true],
  ["#/secrets", "Secrets", true],
  ["#/settings", "Settings", false],
  ["#/settings/gateway", "Settings", false],
];

test("each route hash mounts its view and moves aria-current with it", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  for (const [hash, title, isStub] of ROUTES) {
    navigate(dom, hash);
    await settle(2);
    const heading = root.querySelector("main h1.view-title");
    assert.equal(heading?.textContent, title, `${hash} mounts the ${title} view`);
    if (isStub) {
      assert.ok(
        root.querySelector("main .view-empty"),
        `${hash} renders the stub's empty state`,
      );
    }
    const current = root.querySelectorAll(".tab[aria-current='page']");
    assert.equal(current.length, 1, `${hash} marks exactly one tab current`);
    assert.equal(current[0].textContent, title, `${hash} underlines the ${title} tab`);
  }
});

test("an unknown hash falls back to the models view", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/no-such-view");
  await settle(2);
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Models");
  assert.equal(dom.window.location.hash, "#/models", "the address bar is normalized");
});

test("the removed Downloads tab and route cannot be reached", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  assert.equal(
    [...root.querySelectorAll(".tab")].some((tab) => tab.textContent === "Downloads"),
    false,
    "the tab bar has no Downloads destination",
  );
  navigate(dom, "#/downloads");
  await settle(2);
  assert.equal(dom.window.location.hash, "#/models", "the old route normalizes to Models");
});

test("a malformed percent-escape in the hash falls back instead of crashing", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });
  navigate(dom, "#/models/%");
  await settle(2);
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Models");
  assert.equal(dom.window.location.hash, "#/models", "the bad hash is normalized");
});

test("booting with no hash lands on the models view", async () => {
  const stub = gatewayStub();
  const { root } = await bootApp({ key: "k", stub });
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Models");
  const current = root.querySelectorAll(".tab[aria-current='page']");
  assert.equal(current.length, 1);
  assert.equal(current[0].textContent, "Models");
});
