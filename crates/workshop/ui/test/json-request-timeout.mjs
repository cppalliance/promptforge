// Unit test for the route-timeout rendering in the shared HTTP floor
// (src/services/json-request.ts) and its adoption at the write boundary
// (src/services/workspace-api.ts). Bundles the TS modules with esbuild and
// imports them via a data URL. Covers: a 408 whose body is the JSON error
// envelope yields the envelope's message and the `deadline_elapsed` code; a
// 408 whose body is empty reads as `null` and renders a readable timeout
// error, never the non-JSON-answer shape failure.
// Run: node --test test/json-request-timeout.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as jsonRequest from "./src/services/json-request.ts";
      export * as workspace from "./src/services/workspace-api.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const code = bundle.outputFiles[0].text;
const { jsonRequest, workspace } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The server's deadline envelope, as support/deadline.rs answers it.
const TIMEOUT_MESSAGE =
  "the request did not finish within its 10s deadline; the operation may still complete";
const ENVELOPE = { error: { message: TIMEOUT_MESSAGE, code: "deadline_elapsed" } };

const jsonResponse = (status, body) => ({
  ok: status >= 200 && status < 300,
  status,
  json: async () => body,
});

// An empty 408 body: json() rejects the way a real empty Response does.
const emptyResponse = {
  ok: false,
  status: 408,
  json: async () => {
    throw new SyntaxError("unexpected end of JSON input");
  },
};

// Scripts globalThis.fetch per case: `respond` is a fake Response.
function withFetch(respond, run) {
  const previous = globalThis.fetch;
  globalThis.fetch = async () => respond;
  return run().finally(() => {
    globalThis.fetch = previous;
  });
}

// --- The shared floor: a 408 JSON envelope yields message and code ---------

{
  check(
    "a 408 JSON envelope yields the envelope's message",
    jsonRequest.errorMessage(ENVELOPE, 408, "PUT /workspace/file") === TIMEOUT_MESSAGE,
  );
  check(
    "a 408 JSON envelope yields the deadline_elapsed code",
    jsonRequest.errorCode(ENVELOPE) === "deadline_elapsed",
  );
}

// --- The shared floor: an empty 408 body reads as null, renders a timeout --

{
  const body = await jsonRequest.readJson(emptyResponse, "PUT /workspace/file");
  check("an empty 408 body reads as null", body === null);
  check(
    "an empty 408 body renders a readable timeout, not a non-JSON answer",
    jsonRequest.errorMessage(body, 408, "PUT /workspace/file") === "PUT /workspace/file timed out",
  );
}

// --- Through the write boundary: the envelope message reaches the caller ---

await withFetch(jsonResponse(408, ENVELOPE), async () => {
  let caught = null;
  try {
    await workspace.writeFile("/tmp/note.txt", "late write", null);
  } catch (error) {
    caught = error;
  }
  check(
    "a write's 408 envelope keeps the server's timeout message",
    caught !== null && caught.message === TIMEOUT_MESSAGE,
  );
  check("a write's 408 envelope keeps the status", caught !== null && caught.status === 408);
});

await withFetch(emptyResponse, async () => {
  let caught = null;
  try {
    await workspace.writeFile("/tmp/note.txt", "late write", null);
  } catch (error) {
    caught = error;
  }
  check(
    "a write's empty 408 body renders a readable timeout",
    caught !== null && caught.message === "PUT /workspace/file timed out",
  );
  check(
    "a write's empty 408 body never reports a non-JSON answer",
    caught !== null && !caught.message.includes("non-JSON"),
  );
});

if (failures.length > 0) {
  console.error(`json-request-timeout: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("json-request-timeout: all assertions passed");
