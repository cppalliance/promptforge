import assert from "node:assert/strict";
import test from "node:test";

import { bootApp, envFixture, gatewayStub, modelsFixture, navigate, settle } from "../harness.mjs";

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
