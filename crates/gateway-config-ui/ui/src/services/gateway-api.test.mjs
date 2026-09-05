// Pins the refusal threading every route shares: a refusal from a
// route other than config-apply throws a GatewayHttpError carrying the
// envelope's `error.code` alongside its message, so callers can branch
// on the code (the way the shell words the apply_cancelled toast).
import assert from "node:assert/strict";
import test from "node:test";

import { jsonResponse, loadApp } from "../harness.mjs";

/** A minimal Storage stand-in holding one verified key. */
function storageShim() {
  const map = new Map([["gateway-api-key", "k"]]);
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
    removeItem: (key) => map.delete(key),
  };
}

test("a putConfig refusal carries the envelope's error code", async () => {
  const app = await loadApp();
  const fetchFn = async () =>
    jsonResponse(
      {
        error: {
          message: "unknown field `typo`",
          type: "invalid_request_error",
          code: "config_invalid",
        },
      },
      422,
    );
  const api = new app.GatewayApi({ fetchFn, storage: storageShim() });

  await assert.rejects(api.putConfig({}), (error) => {
    assert.ok(error instanceof app.GatewayHttpError, "the refusal throws GatewayHttpError");
    assert.equal(error.status, 422);
    assert.equal(error.message, "unknown field `typo`", "the envelope message is kept");
    assert.equal(error.code, "config_invalid", "the envelope code rides the error");
    return true;
  });
});
