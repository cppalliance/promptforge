// Pins the sheet store's poll cycle: it requests the cloud model sheet
// on start, keeps polling while the gateway answers its loading
// indication, notifies once when the sheet lands, stops polling on a
// terminal download error, and re-requests on refresh.
import assert from "node:assert/strict";
import test from "node:test";

import { cloudSheetFixture, jsonResponse, loadApp } from "../harness.mjs";

const app = await loadApp();

/** A minimal Storage stand-in holding one verified key. */
function storageShim() {
  const map = new Map([["gateway-api-key", "k"]]);
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
  };
}

/** A GatewayApi over a handler, with every call recorded. */
function apiWith(handler) {
  const calls = [];
  const fetchFn = async (input, init = {}) => {
    calls.push({ url: String(input), init });
    return handler(String(input), init);
  };
  return { api: new app.GatewayApi({ fetchFn, storage: storageShim() }), calls };
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const LOADING = () =>
  jsonResponse({ error: { message: "the sheet is downloading", code: "cloud_models_loading" } }, 503);

test("polls while loading until the sheet arrives, notifying once per transition", async () => {
  let gets = 0;
  const { api, calls } = apiWith(() => {
    gets += 1;
    return gets < 3 ? LOADING() : jsonResponse(cloudSheetFixture());
  });
  const store = new app.SheetStore(api, 5);
  const notifications = [];
  store.subscribe(() => notifications.push(store.status));
  store.start();
  await sleep(200);
  assert.equal(store.status, "loaded");
  assert.equal(store.sheet.providers.anthropic.display_name, "Anthropic");
  assert.equal(store.error, null);
  assert.deepEqual(notifications, ["loading", "loaded"]);
  store.dispose();
  const settled = calls.length;
  await sleep(30);
  assert.equal(calls.length, settled, "no polling continues after the sheet arrives");
});

test("a download error stops polling and notifies the error", async () => {
  const { api, calls } = apiWith(() =>
    jsonResponse({ error: { message: "the download failed", code: "cloud_models_unavailable" } }, 502),
  );
  const store = new app.SheetStore(api, 5);
  const notifications = [];
  store.subscribe(() => notifications.push(store.status));
  store.start();
  await sleep(50);
  assert.equal(store.status, "error");
  assert.equal(store.error, "the download failed");
  assert.equal(store.sheet, null);
  assert.deepEqual(notifications, ["loading", "error"]);
  assert.equal(calls.length, 1, "a terminal error is not retried");
  store.dispose();
});

test("refresh forces a re-download and notifies again when the sheet lands", async () => {
  let loading = true;
  const { api, calls } = apiWith((url, init) => {
    if (url.endsWith("/admin/cloud-models/refresh")) {
      return jsonResponse({}, 202);
    }
    if (loading) {
      loading = false;
      return LOADING();
    }
    return jsonResponse(cloudSheetFixture());
  });
  const store = new app.SheetStore(api, 5);
  let notifications = 0;
  store.subscribe(() => {
    notifications += 1;
  });
  store.start();
  await sleep(100);
  assert.equal(store.status, "loaded");
  const baseline = notifications;
  await store.refresh();
  await sleep(100);
  assert.ok(
    calls.some(
      (call) => call.url.endsWith("/admin/cloud-models/refresh") && call.init.method === "POST",
    ),
    "the refresh posts to the refresh route",
  );
  assert.ok(notifications > baseline, "the re-requested sheet notifies again");
  assert.equal(store.status, "loaded");
  store.dispose();
});

test("a failed refresh POST records the error and rejects so the caller can surface it", async () => {
  const { api } = apiWith((url) => {
    if (url.endsWith("/admin/cloud-models/refresh")) {
      return jsonResponse({ error: { message: "refresh boom", code: "cloud_models_unavailable" } }, 502);
    }
    return jsonResponse(cloudSheetFixture());
  });
  const store = new app.SheetStore(api, 5);
  const notifications = [];
  store.subscribe(() => notifications.push(store.status));
  store.start();
  await sleep(50);
  assert.equal(store.status, "loaded");
  await assert.rejects(store.refresh(), /refresh boom/, "the refresh failure reaches the caller");
  assert.equal(store.status, "error");
  assert.equal(store.error, "refresh boom");
  assert.ok(store.sheet !== null, "the loaded sheet stays in place");
  assert.deepEqual(notifications, ["loading", "loaded", "error"]);
  store.dispose();
});
