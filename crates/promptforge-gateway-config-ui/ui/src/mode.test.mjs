// Pins mode detection at boot: ?mode=panel mounts the shell without
// the key prompt or the medallion (and calls no gateway API until the
// workshop bridge exists), while standalone shows the medallion and the
// skip link once a stored key admits the shell.
import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, gatewayStub } from "./harness.mjs";

test("?mode=panel skips the key prompt and the medallion", async () => {
  const stub = gatewayStub();
  const { root } = await bootApp({
    url: "http://127.0.0.1:8081/config/?mode=panel",
    stub,
  });

  assert.equal(root.querySelector("#gateway-api-key"), null, "no key prompt in panel mode");
  assert.ok(root.querySelector("header.tab-bar"), "the tab bar mounted");
  assert.equal(root.querySelector("img.tab-medallion"), null, "no medallion in panel mode");
  assert.equal(
    root.querySelectorAll("nav[aria-label='Primary'] a.tab").length,
    6,
    "all six tabs are present",
  );
  assert.match(
    root.querySelector(".banner")?.textContent ?? "",
    /bridge pending/i,
    "the shell notes the workshop bridge is pending",
  );
  assert.equal(root.querySelector("main h1.view-title")?.textContent, "Models");
  assert.equal(stub.calls.length, 0, "panel mode calls no gateway API before the bridge");
});

test("standalone mode shows the medallion and a working skip link", async () => {
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ key: "k", stub });

  const medallion = root.querySelector("img.tab-medallion");
  assert.ok(medallion, "the medallion is shown standalone");
  assert.equal(medallion.getAttribute("width"), "24");
  assert.equal(medallion.getAttribute("height"), "24");

  const skip = root.querySelector("a.skip-link");
  assert.ok(skip, "the skip link is present");
  const main = root.querySelector("main#main");
  skip.click();
  assert.equal(dom.window.document.activeElement, main, "the skip link focuses main");
  assert.equal(dom.window.location.hash, "#/models", "the skip jump never rewrites the route");
});

test("the connection dot goes green after a successful API call", async () => {
  const stub = gatewayStub();
  const { root } = await bootApp({ key: "k", stub });
  const dot = root.querySelector(".status-dot");
  assert.ok(dot.classList.contains("is-ok"), "the dot reflects the reachable gateway");
});

test("the connection dot goes red when the gateway is unreachable", async () => {
  const stub = { fetchFn: () => Promise.reject(new TypeError("network down")) };
  const { root } = await bootApp({ key: "k", stub });
  const dot = root.querySelector(".status-dot");
  assert.ok(dot.classList.contains("is-bad"), "the dot reflects the unreachable gateway");
});
