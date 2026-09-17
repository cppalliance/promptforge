// Unit test for the typed error catalog (src/services/error-catalog.ts) and
// its adoption at the HTTP boundaries (src/services/workspace-api.ts,
// src/ui/workspace/workspace-drops.ts). Bundles the TS modules with esbuild
// and imports them via a data URL. Covers: the Result constructors and the
// CatalogError shape; the shared errorText narrowing; the workspace API
// throwing typed variants for transport, HTTP, shape, and conflict
// failures; isModifiedConflict keying on the catalog code rather than a
// class; and grantPath returning a typed Result instead of throwing.
// Run: node test/error-catalog.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as catalog from "./src/services/error-catalog.ts";
      export * as workspace from "./src/services/workspace-api.ts";
      export { grantPath } from "./src/ui/workspace/workspace-drops.ts";
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
const { catalog, workspace, grantPath } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const { ErrorCatalog, CatalogError } = catalog;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Scripts globalThis.fetch per case: `respond` is a fake Response or a
// function throwing a transport failure.
function withFetch(respond, run) {
  const previous = globalThis.fetch;
  globalThis.fetch =
    respond instanceof Error
      ? async () => {
          throw respond;
        }
      : async () => respond;
  return run().finally(() => {
    globalThis.fetch = previous;
  });
}

const jsonResponse = (status, body) => ({
  ok: status >= 200 && status < 300,
  status,
  json: async () => body,
});

// --- The Result constructors ------------------------------------------------

{
  const success = catalog.ok(42);
  check("ok carries the value", success.ok === true && success.value === 42);
  const failure = catalog.err(new CatalogError(ErrorCatalog.Transport, "down"));
  check("err carries the error", failure.ok === false && failure.error.code === "transport");
}

// --- The CatalogError shape ---------------------------------------------------

{
  const error = new CatalogError(ErrorCatalog.HttpStatus, "GET /x answered 500", {
    status: 500,
  });
  check("a catalog error is an Error", error instanceof Error);
  check("a catalog error keeps its message", error.message === "GET /x answered 500");
  check("a catalog error carries its code", error.code === ErrorCatalog.HttpStatus);
  check("a catalog error carries the HTTP status", error.status === 500);
  check("a catalog error names itself", error.name === "CatalogError");
  check(
    "isCatalogError narrows by code",
    catalog.isCatalogError(error, ErrorCatalog.HttpStatus) &&
      !catalog.isCatalogError(error, ErrorCatalog.Transport),
  );
  check(
    "isCatalogError refuses plain Errors",
    !catalog.isCatalogError(new Error("GET /x answered 500")),
  );
}

// --- errorText: the one narrowing helper --------------------------------------

{
  check(
    "errorText reads an Error's message",
    catalog.errorText(new Error("plain")) === "plain",
  );
  check(
    "errorText reads a foreign object's message",
    catalog.errorText({ message: "foreign" }) === "foreign",
  );
  check("errorText falls back to String()", catalog.errorText(42) === "42");
}

// --- workspace-api: typed transport, HTTP, and shape failures -----------------

await withFetch(new Error("connection refused"), async () => {
  let caught = null;
  try {
    await workspace.fetchTree(null);
  } catch (error) {
    caught = error;
  }
  check("a transport failure throws a CatalogError", caught instanceof CatalogError);
  check(
    "a transport failure carries the transport code",
    caught !== null && caught.code === ErrorCatalog.Transport,
  );
  check(
    "a transport failure keeps the cause's message",
    caught !== null && caught.message.includes("connection refused"),
  );
});

await withFetch(
  jsonResponse(500, { error: { message: "disk full", code: "internal" } }),
  async () => {
    let caught = null;
    try {
      await workspace.fetchTree(null);
    } catch (error) {
      caught = error;
    }
    check("an HTTP failure throws a CatalogError", caught instanceof CatalogError);
    check(
      "an HTTP failure carries the http_status code and status",
      caught !== null && caught.code === ErrorCatalog.HttpStatus && caught.status === 500,
    );
    check(
      "an HTTP failure keeps the server's message",
      caught !== null && caught.message === "disk full",
    );
  },
);

await withFetch(jsonResponse(200, { unexpected: true }), async () => {
  let caught = null;
  try {
    await workspace.fetchTree(null);
  } catch (error) {
    caught = error;
  }
  check(
    "a shape failure carries the unexpected_shape code",
    caught instanceof CatalogError && caught.code === ErrorCatalog.UnexpectedShape,
  );
});

// --- writeFile: the conflict is a catalog code, not a class --------------------

await withFetch(
  jsonResponse(409, { error: { message: "the file changed on disk", code: "modified_conflict" } }),
  async () => {
    let caught = null;
    try {
      await workspace.writeFile("/tmp/a.md", "text", null);
    } catch (error) {
      caught = error;
    }
    check(
      "a modified conflict is a CatalogError with the conflict code",
      caught instanceof CatalogError && caught.code === ErrorCatalog.ModifiedConflict,
    );
    check("isModifiedConflict recognizes the conflict", workspace.isModifiedConflict(caught));
  },
);

await withFetch(
  jsonResponse(409, { error: { message: "some other refusal", code: "other" } }),
  async () => {
    let caught = null;
    try {
      await workspace.writeFile("/tmp/a.md", "text", null);
    } catch (error) {
      caught = error;
    }
    check(
      "a non-conflict 409 is a plain HTTP failure",
      caught instanceof CatalogError && caught.code === ErrorCatalog.HttpStatus,
    );
    check(
      "isModifiedConflict refuses a non-conflict failure",
      !workspace.isModifiedConflict(caught),
    );
  },
);

// --- grantPath: a typed Result instead of a throw ------------------------------

await withFetch(jsonResponse(200, { granted: "/tmp/x" }), async () => {
  const result = await grantPath("/tmp/x");
  check("a granted path resolves ok", result.ok === true);
});

await withFetch(
  jsonResponse(403, { error: { message: "path is outside every granted root" } }),
  async () => {
    const result = await grantPath("/tmp/blocked");
    check("a refused grant resolves err, never throws", result.ok === false);
    check(
      "a refused grant carries the grant_refused code",
      result.ok === false && result.error.code === ErrorCatalog.GrantRefused,
    );
    check(
      "a refused grant keeps the server's message",
      result.ok === false && result.error.message === "path is outside every granted root",
    );
  },
);

await withFetch(new Error("socket hangup"), async () => {
  const result = await grantPath("/tmp/x");
  check(
    "a grant transport failure resolves err with the transport code",
    result.ok === false && result.error.code === ErrorCatalog.Transport,
  );
});

if (failures.length > 0) {
  console.error(`error-catalog: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("error-catalog: all assertions passed");
