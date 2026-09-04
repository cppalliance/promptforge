// Pins the key prompt flow: a verified key lands in sessionStorage and
// mounts the shell, a rejected key shows the inline error without
// storing anything, and a 401 from any later API call clears the key
// and returns to the prompt.
import assert from "node:assert/strict";
import test from "node:test";

import {
  bootApp,
  gatewayStub,
  jsonResponse,
  loadApp,
  makeDom,
  modelsFixture,
  settle,
} from "../harness.mjs";

/** Fills the key input and fires the form's submit handler. */
function submitKey(dom, root, value) {
  const input = root.querySelector("#gateway-api-key");
  input.value = value;
  const form = root.querySelector("form");
  form.dispatchEvent(new dom.window.Event("submit", { bubbles: true, cancelable: true }));
}

test("a verified key is stored in sessionStorage and the shell mounts", async () => {
  const app = await loadApp();
  const stub = gatewayStub({ key: "sesame" });
  const { dom, root } = await bootApp({ stub });

  const label = root.querySelector("label[for='gateway-api-key']");
  assert.equal(label?.textContent, "API key", "the input carries a real label");

  submitKey(dom, root, "sesame");
  await settle();

  assert.equal(
    dom.window.sessionStorage.getItem(app.API_KEY_STORAGE_KEY),
    "sesame",
    "the verified key is stored for the session",
  );
  assert.ok(root.querySelector("header.tab-bar"), "the shell mounted after verification");
  assert.equal(root.querySelector("#gateway-api-key"), null, "the prompt is gone");
});

test("an ambient handoff cookie mounts the shell without a stored key", async () => {
  // No key in the stub: the gateway's /auth cookie authenticates every
  // call, so boot's ambient probe answers 200 and the prompt never shows.
  const app = await loadApp();
  const stub = gatewayStub();
  const { dom, root } = await bootApp({ stub });

  assert.ok(root.querySelector("header.tab-bar"), "the shell mounted on the ambient cookie");
  assert.equal(root.querySelector("#gateway-api-key"), null, "no key prompt is shown");
  assert.equal(dom.window.sessionStorage.getItem(app.API_KEY_STORAGE_KEY), null, "no key is stored");
});

test("a rejected key shows the inline invalid-key error and stores nothing", async () => {
  const app = await loadApp();
  const stub = gatewayStub({ key: "sesame" });
  const { dom, root } = await bootApp({ stub });

  submitKey(dom, root, "wrong");
  await settle();

  const error = root.querySelector("#gateway-api-key-error");
  assert.ok(error && !error.hidden, "the inline error is visible");
  assert.equal(error.textContent, "Invalid API key");
  const input = root.querySelector("#gateway-api-key");
  assert.equal(input.getAttribute("aria-invalid"), "true");
  assert.equal(input.getAttribute("aria-describedby"), "gateway-api-key-error");
  assert.equal(dom.window.sessionStorage.getItem(app.API_KEY_STORAGE_KEY), null);
  assert.equal(root.querySelector("header.tab-bar"), null, "the shell did not mount");
});

test("a 401 from any later API call clears the key and returns to the prompt", async () => {
  const app = await loadApp();
  const gateway = gatewayStub({ config: modelsFixture() });
  let authorized = true;
  const stub = {
    fetchFn: (url, init) =>
      authorized
        ? gateway.fetchFn(url, init)
        : Promise.resolve(jsonResponse({ error: "unauthorized" }, 401)),
  };
  const { dom, root } = await bootApp({ key: "k", stub });
  assert.ok(root.querySelector("header.tab-bar"), "the shell mounted with the stored key");

  authorized = false;
  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "travel")
    .click();
  await settle();

  assert.ok(root.querySelector("#gateway-api-key"), "the key prompt is back");
  assert.equal(root.querySelector("header.tab-bar"), null, "the shell is gone");
  assert.equal(
    dom.window.sessionStorage.getItem(app.API_KEY_STORAGE_KEY),
    null,
    "the rejected key was cleared",
  );
});

test("a 401 remount cycle tears down the old router and progress stream", async () => {
  const app = await loadApp();
  const dom = makeDom();
  const gateway = gatewayStub({ config: modelsFixture() });
  let authorized = true;
  const fetchFn = (url, init) =>
    authorized
      ? gateway.fetchFn(url, init)
      : Promise.resolve(jsonResponse({ error: "unauthorized" }, 401));

  // A counting wrapper pins that every mounted router is later removed.
  let added = 0;
  let removed = 0;
  const win = {
    location: dom.window.location,
    sessionStorage: dom.window.sessionStorage,
    addEventListener: (type, listener) => {
      if (type === "hashchange") {
        added += 1;
      }
      dom.window.addEventListener(type, listener);
    },
    removeEventListener: (type, listener) => {
      if (type === "hashchange") {
        removed += 1;
      }
      dom.window.removeEventListener(type, listener);
    },
  };

  dom.window.sessionStorage.setItem(app.API_KEY_STORAGE_KEY, "k");
  const root = dom.window.document.createElement("div");
  dom.window.document.body.append(root);
  app.boot(root, { win, fetchFn });
  await settle();
  assert.ok(root.querySelector("header.tab-bar"), "the shell mounted with the stored key");

  authorized = false;
  root.querySelector(".profile-switcher button").click();
  await settle();
  [...root.querySelectorAll("[role='menuitemradio']")]
    .find((row) => row.textContent === "travel")
    .click();
  await settle();
  assert.ok(root.querySelector("#gateway-api-key"), "the 401 returned to the key prompt");

  authorized = true;
  const input = root.querySelector("#gateway-api-key");
  input.value = "k";
  root
    .querySelector("form")
    .dispatchEvent(new dom.window.Event("submit", { bubbles: true, cancelable: true }));
  await settle();
  assert.ok(root.querySelector("header.tab-bar"), "the shell remounted after re-auth");

  assert.equal(added - removed, 1, "exactly one live hashchange listener remains");
  const progressCalls = gateway.calls.filter((call) => call.url.endsWith("/admin/progress"));
  assert.equal(progressCalls.length, 2, "each shell mount opened one progress stream");
  assert.ok(
    progressCalls[0].init.signal.aborted,
    "the first shell's progress stream was aborted on teardown",
  );
});
