// Unit test for the UI-state adapter (src/services/ui-storage.ts): the
// SPA's replacement for localStorage, a two-bucket key-value cache fed by
// GET /user/state and GET /workspace/file/state at boot and written back
// through PUT /user/state/{key} and PUT /workspace/file/state/{key}.
// Bundles the module with esbuild and drives it with an injected fake
// fetch. Covers: both buckets resolving and `get` answering each value;
// one bucket failing (rejection, non-OK status, non-object body) with only
// its keys null and one warning; both buckets hanging past the preload
// timeout with boot still completing on all-null; a fetch that fails after
// the timeout adding no second warning; `set` issuing the PUT
// with the JSON body to the right path and updating the cache; a rejected
// or refused `set` warning once without throwing; `suppressWrites`
// dropping workspace writes but not user writes, nesting safely, and
// restoring after a throw; `reloadWorkspace` replacing the cached
// workspace values; and the UI_STORAGE token's default empty adapter.
// Run: node test/ui-storage.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createUiStorage, UI_STORAGE } from "./src/services/ui-storage.ts";
      export { getService } from "./src/services/service-registry.ts";
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
const { createUiStorage, UI_STORAGE, getService } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

// Counts console warnings while a body runs; the adapter's failure
// posture is "one warning, continue", so the count is part of the contract.
const realWarn = console.warn;
let warnings = [];
console.warn = (...args) => {
  warnings.push(args.map(String).join(" "));
};
function resetWarnings() {
  warnings = [];
}

const jsonResponse = (body, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

const USER_ROUTE = "/user/state";
const WORKSPACE_ROUTE = "/workspace/file/state";

/**
 * A scripted fetch: `script` maps a route to a behavior, one of
 * `{ body }` (200 JSON), `{ status, body }`, `{ reject: true }`,
 * `{ rejectAfter: ms }` (rejects after a delay), or `{ hang: true }`. PUT
 * routes default to `{ saved: true }`. Every call is recorded as
 * `{ method, url, body }` with the body parsed from JSON.
 */
function scriptedFetch(script) {
  const calls = [];
  const fetchImpl = (url, init) => {
    const method = init?.method ?? "GET";
    calls.push({
      method,
      url,
      headers: init?.headers ?? {},
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
    });
    const behavior = script[url] ?? (method === "PUT" ? { body: { saved: true } } : undefined);
    if (behavior === undefined) {
      return Promise.reject(new Error(`unexpected fetch ${method} ${url}`));
    }
    if (behavior.hang) {
      return new Promise(() => {});
    }
    if (behavior.reject) {
      return Promise.reject(new Error("connection refused"));
    }
    if (behavior.rejectAfter !== undefined) {
      return new Promise((_, reject) =>
        setTimeout(() => reject(new Error("connection reset")), behavior.rejectAfter),
      );
    }
    return Promise.resolve(jsonResponse(behavior.body, behavior.status ?? 200));
  };
  return { fetchImpl, calls };
}

const userState = {
  editor_settings: { wordWrap: true },
  zoom: 1.25,
  recent_files: ["C:\\a.md"],
  commands_history: null,
};
const workspaceState = {
  layout: { version: 3, zones: {} },
  tree: { expanded: ["C:\\project"] },
  closed_editors: null,
};

// --- Both buckets resolve ---------------------------------------------------

{
  resetWarnings();
  const { fetchImpl, calls } = scriptedFetch({
    [USER_ROUTE]: { body: userState },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  });
  const storage = createUiStorage(fetchImpl);
  check("get before preload answers null", storage.get("user", "zoom") === null);
  await storage.preload(1000);
  check(
    "preload fetches both buckets",
    calls.length === 2 &&
      calls.some((c) => c.method === "GET" && c.url === USER_ROUTE) &&
      calls.some((c) => c.method === "GET" && c.url === WORKSPACE_ROUTE),
  );
  check(
    "get answers each user value",
    storage.get("user", "zoom") === 1.25 &&
      storage.get("user", "editor_settings")?.wordWrap === true &&
      storage.get("user", "recent_files")?.[0] === "C:\\a.md",
  );
  check(
    "get answers each workspace value",
    storage.get("workspace", "layout")?.version === 3 &&
      storage.get("workspace", "tree")?.expanded?.[0] === "C:\\project",
  );
  check(
    "a null server value reads as null",
    storage.get("user", "commands_history") === null && storage.get("workspace", "closed_editors") === null,
  );
  check("an unknown key reads as null", storage.get("user", "nope") === null);
  check("a clean preload warns nothing", warnings.length === 0);
}

// --- One bucket fails --------------------------------------------------------

{
  resetWarnings();
  const { fetchImpl } = scriptedFetch({
    [USER_ROUTE]: { reject: true },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  check("a rejected user bucket reads all null", storage.get("user", "zoom") === null);
  check("the workspace bucket still holds its values", storage.get("workspace", "layout")?.version === 3);
  check("a rejected bucket warns exactly once", warnings.length === 1);
}

{
  resetWarnings();
  const { fetchImpl } = scriptedFetch({
    [USER_ROUTE]: { body: userState },
    [WORKSPACE_ROUTE]: { status: 500, body: { error: { code: "internal", message: "boom" } } },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  check("a non-OK workspace bucket reads all null", storage.get("workspace", "layout") === null);
  check("the user bucket still holds its values", storage.get("user", "zoom") === 1.25);
  check("a non-OK bucket warns exactly once", warnings.length === 1);
}

{
  resetWarnings();
  const { fetchImpl } = scriptedFetch({
    [USER_ROUTE]: { body: ["not", "an", "object"] },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  check("a non-object body reads all null", storage.get("user", "zoom") === null);
  check("a non-object body warns exactly once", warnings.length === 1);
}

// --- Both buckets hang past the timeout ---------------------------------------

{
  resetWarnings();
  const { fetchImpl } = scriptedFetch({
    [USER_ROUTE]: { hang: true },
    [WORKSPACE_ROUTE]: { hang: true },
  });
  const storage = createUiStorage(fetchImpl);
  const started = Date.now();
  await storage.preload(30);
  check("preload resolves after the timeout", Date.now() - started < 1000);
  check(
    "a timed-out preload reads all null",
    storage.get("user", "zoom") === null && storage.get("workspace", "layout") === null,
  );
  check("a timed-out preload warns once per bucket", warnings.length === 2);
}

// --- A fetch that fails after the timeout is not a second warning ---------------

{
  resetWarnings();
  const { fetchImpl } = scriptedFetch({
    [USER_ROUTE]: { rejectAfter: 80 },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(20);
  check("the timed-out bucket reads null", storage.get("user", "zoom") === null);
  check("the other bucket holds its values", storage.get("workspace", "layout")?.version === 3);
  await new Promise((resolve) => setTimeout(resolve, 150));
  check("a late failure after the timeout still warns exactly once", warnings.length === 1);
}

// --- set PUTs to the right path ------------------------------------------------

{
  resetWarnings();
  const { fetchImpl, calls } = scriptedFetch({
    [USER_ROUTE]: { body: userState },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  calls.length = 0;

  storage.set("user", "zoom", 1.5);
  const userPut = calls.find((c) => c.method === "PUT");
  check(
    "a user set PUTs the JSON body to /user/state/{key}",
    userPut !== undefined && userPut.url === `${USER_ROUTE}/zoom` && userPut.body === 1.5,
  );
  check(
    "the PUT declares a JSON content type",
    userPut !== undefined && /application\/json/.test(String(userPut.headers["content-type"])),
  );
  check("a set updates the cache before the PUT settles", storage.get("user", "zoom") === 1.5);

  calls.length = 0;
  const layout = { version: 3, zones: { tree: true } };
  storage.set("workspace", "layout", layout);
  const workspacePut = calls.find((c) => c.method === "PUT");
  check(
    "a workspace set PUTs the JSON body to /workspace/file/state/{key}",
    workspacePut !== undefined &&
      workspacePut.url === `${WORKSPACE_ROUTE}/layout` &&
      workspacePut.body?.zones?.tree === true,
  );
  await settle();
  check("a successful set warns nothing", warnings.length === 0);
}

// --- A failed set warns once and does not throw ----------------------------------

{
  resetWarnings();
  const { fetchImpl } = scriptedFetch({
    [USER_ROUTE]: { body: userState },
    [WORKSPACE_ROUTE]: { body: workspaceState },
    [`${USER_ROUTE}/zoom`]: { reject: true },
    [`${WORKSPACE_ROUTE}/tree`]: { status: 400, body: { error: { code: "bad_key", message: "no" } } },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  let threw = false;
  try {
    storage.set("user", "zoom", 2);
  } catch {
    threw = true;
  }
  await settle();
  check("a rejected set does not throw", !threw);
  check("a rejected set warns once", warnings.length === 1);
  check("a rejected set leaves the in-memory value", storage.get("user", "zoom") === 2);

  resetWarnings();
  storage.set("workspace", "tree", { expanded: [] });
  await settle();
  check("a refused set warns once", warnings.length === 1);
}

// --- suppressWrites ---------------------------------------------------------------

{
  resetWarnings();
  const { fetchImpl, calls } = scriptedFetch({
    [USER_ROUTE]: { body: userState },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  });
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  calls.length = 0;

  const result = storage.suppressWrites(() => {
    storage.set("workspace", "layout", { version: 3 });
    storage.set("user", "zoom", 3);
    storage.suppressWrites(() => {
      storage.set("workspace", "tree", { expanded: [] });
    });
    storage.set("workspace", "closed_editors", { paths: [] });
    return "done";
  });
  const puts = calls.filter((c) => c.method === "PUT");
  check("suppressWrites passes the callback's return through", result === "done");
  check(
    "suppressWrites drops every workspace write, nested included",
    puts.every((c) => !c.url.startsWith(WORKSPACE_ROUTE)),
  );
  check(
    "suppressWrites lets user writes through",
    puts.length === 1 && puts[0].url === `${USER_ROUTE}/zoom`,
  );

  calls.length = 0;
  storage.set("workspace", "layout", { version: 3 });
  check(
    "workspace writes resume after suppressWrites",
    calls.some((c) => c.method === "PUT" && c.url === `${WORKSPACE_ROUTE}/layout`),
  );

  calls.length = 0;
  try {
    storage.suppressWrites(() => {
      throw new Error("apply failed");
    });
  } catch {
    // expected
  }
  storage.set("workspace", "tree", { expanded: ["x"] });
  check(
    "suppression lifts even when the callback throws",
    calls.some((c) => c.method === "PUT" && c.url === `${WORKSPACE_ROUTE}/tree`),
  );
  await settle();
}

// --- reloadWorkspace ----------------------------------------------------------------

{
  resetWarnings();
  const script = {
    [USER_ROUTE]: { body: userState },
    [WORKSPACE_ROUTE]: { body: workspaceState },
  };
  const { fetchImpl, calls } = scriptedFetch(script);
  const storage = createUiStorage(fetchImpl);
  await storage.preload(1000);
  calls.length = 0;

  script[WORKSPACE_ROUTE] = {
    body: { layout: { version: 3, zones: { other: true } }, tree: null, closed_editors: { paths: ["C:\\b.md"] } },
  };
  await storage.reloadWorkspace();
  check(
    "reloadWorkspace fetches only the workspace bucket",
    calls.length === 1 && calls[0].method === "GET" && calls[0].url === WORKSPACE_ROUTE,
  );
  check(
    "reloadWorkspace replaces the cached workspace values",
    storage.get("workspace", "layout")?.zones?.other === true &&
      storage.get("workspace", "tree") === null &&
      storage.get("workspace", "closed_editors")?.paths?.[0] === "C:\\b.md",
  );
  check("reloadWorkspace leaves the user bucket alone", storage.get("user", "zoom") === 1.25);

  script[WORKSPACE_ROUTE] = { reject: true };
  await storage.reloadWorkspace();
  check("a failed reload yields defaults", storage.get("workspace", "layout") === null);
  check("a failed reload warns once", warnings.length === 1);
}

// --- The default empty adapter --------------------------------------------------------

{
  resetWarnings();
  const fallback = getService(UI_STORAGE);
  check("the UI_STORAGE token self-registers", fallback !== null && typeof fallback.get === "function");
  check("the empty adapter reads null", fallback.get("user", "zoom") === null);
  let threw = false;
  try {
    fallback.set("user", "zoom", 1);
    fallback.set("workspace", "layout", {});
  } catch {
    threw = true;
  }
  check("the empty adapter's set is a no-op", !threw && fallback.get("user", "zoom") === null);
  await fallback.preload(5);
  await fallback.reloadWorkspace();
  check("the empty adapter's suppressWrites runs the callback", fallback.suppressWrites(() => 7) === 7);
  check("the empty adapter warns nothing", warnings.length === 0);
}

console.warn = realWarn;

if (failures.length > 0) {
  console.error(`ui-storage: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ui-storage: all assertions passed");
process.exit(0);
