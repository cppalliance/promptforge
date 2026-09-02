// Pins the tab bar's version label: the right-side actions cluster shows
// the baked crate version (or the dev fallback) as a muted `vX.Y.Z` span.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub, modelsFixture } from "../harness.mjs";

test("the tab bar shows the baked version in the actions cluster", async () => {
  const stub = gatewayStub({ key: "k", config: modelsFixture() });
  const { root } = await bootApp({ key: "k", stub });
  const label = root.querySelector(".tab-bar .tab-actions .tab-version");
  assert.ok(label, "the version label renders in the tab-actions cluster");
  assert.match(
    label.textContent,
    /^v(\d+\.\d+\.\d+|dev)$/,
    "the label shows the baked crate version (or the dev fallback)",
  );
});
