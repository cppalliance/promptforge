// Unit test for the Gateway Config panel's workshop side: the
// window-level postMessage bridge (src/ui/gateway-config-bridge.ts) and
// the iframe host panel (src/ui/workshop/gateway-config-panel.ts).
// Bundles the TS modules with esbuild and drives them in jsdom. Covers:
// origin pinning (a message from a foreign origin is ignored and never
// forwarded), the ready announcement answered with a context message
// (theme + initial route) pinned to the gateway origin, the API-forward
// round trip through a stubbed /gateway/api server route (bearer stays
// server-side by construction - the browser never sees a key), the
// transport-failure answer (status 0), action notifications landing on
// the status bar stub, listener teardown on dispose, and the panel's
// iframe address, sandbox, and origin-failure alert.
// Run: node test/gateway-config-bridge.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const result = await esbuild.build({
  stdin: {
    contents: `
      export { setupGatewayConfigBridge } from "./src/ui/gateway-config-bridge.ts";
      export { GatewayConfigPanel } from "./src/ui/workshop/gateway-config-panel.ts";
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
  // The panel imports its colocated CSS; strip it - the test drives only
  // the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});
const code = result.outputFiles[0].text;
const { setupGatewayConfigBridge, GatewayConfigPanel } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const WORKSHOP_ORIGIN = "http://127.0.0.1:7910";
const GATEWAY_ORIGIN = "http://127.0.0.1:8081";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: `${WORKSHOP_ORIGIN}/`,
});
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

/** Lets queued microtasks and zero-delay timers run. */
async function flush(turns = 10) {
  for (let i = 0; i < turns; i += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// --- A stubbed workshop server: origin probe + proxy route -------------------

const fetched = [];
const fetchFn = async (url, init = {}) => {
  fetched.push({ url, init });
  if (url === "/gateway/origin") {
    return new Response(JSON.stringify({ origin: GATEWAY_ORIGIN }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }
  if (url === "/gateway/api/admin/status") {
    return new Response(JSON.stringify({ profile: "default", models: [] }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }
  if (url === "/gateway/api/admin/system") {
    throw new TypeError("the workshop server is unreachable");
  }
  return new Response(JSON.stringify({ error: { code: "forward_denied" } }), {
    status: 403,
    headers: { "content-type": "application/json" },
  });
};
const proxyCalls = () => fetched.filter((call) => call.url.startsWith("/gateway/api"));

const statusLines = [];
const statusBar = {
  showLocal: (label, severity) => statusLines.push({ label, severity }),
};

const replies = [];
const bridge = setupGatewayConfigBridge({
  statusBar,
  fetchFn,
  win: window,
  reply: (_event, message, targetOrigin) => replies.push({ message, targetOrigin }),
});

function dispatch(data, origin = GATEWAY_ORIGIN) {
  window.dispatchEvent(new window.MessageEvent("message", { data, origin }));
}

// --- Origin pinning: a foreign origin is ignored ------------------------------

dispatch({ type: "pf-api", id: "evil", method: "GET", path: "/admin/status", body: null }, "https://evil.example");
await flush();
check("a message from a foreign origin is never forwarded", proxyCalls().length === 0);
check("a message from a foreign origin is never answered", replies.length === 0);

// --- The ready announcement is answered with the pinned context ---------------

dispatch({ type: "pf-bridge-ready" });
await flush();
check("the ready announcement is answered", replies.length === 1);
check(
  "the answer is a context message carrying theme and initial route",
  replies[0]?.message.type === "pf-context" &&
    replies[0]?.message.theme === "dark" &&
    replies[0]?.message.route === "#/models",
);
check(
  "the context reply pins the gateway origin, never *",
  replies[0]?.targetOrigin === GATEWAY_ORIGIN,
);

// --- The API-forward round trip -----------------------------------------------

dispatch({ type: "pf-api", id: "r1", method: "GET", path: "/admin/status", body: null });
await flush();
check(
  "an api request forwards through the workshop server proxy route",
  proxyCalls().length === 1 && proxyCalls()[0].url === "/gateway/api/admin/status",
);
const apiReply = replies.find((entry) => entry.message.type === "pf-api-result");
check(
  "the proxy's answer rides back with the request id, status, and body",
  apiReply !== undefined &&
    apiReply.message.id === "r1" &&
    apiReply.message.status === 200 &&
    apiReply.message.contentType === "application/json" &&
    apiReply.message.body.includes('"profile":"default"'),
);
check("the api reply pins the gateway origin", apiReply?.targetOrigin === GATEWAY_ORIGIN);

// --- A transport failure answers status 0 --------------------------------------

dispatch({ type: "pf-api", id: "r2", method: "GET", path: "/admin/system", body: null });
await flush();
const failureReply = replies.find((entry) => entry.message.id === "r2");
check(
  "an unreachable workshop server answers status 0, not silence",
  failureReply !== undefined && failureReply.message.status === 0,
);

// --- Action notifications land on the status bar --------------------------------

dispatch({ type: "pf-action", action: "apply" });
dispatch({ type: "pf-action", action: "download-started" });
dispatch({ type: "pf-action", action: "reboot-the-host" });
await flush();
check(
  "apply and download-started reach the status bar as local info lines",
  statusLines.map((line) => line.label).join("|") ===
    "Gateway configuration applied|Gateway download started" &&
    statusLines.every((line) => line.severity === "info"),
);

// --- Dispose detaches the listener ----------------------------------------------

const repliesBefore = replies.length;
bridge.dispose();
dispatch({ type: "pf-bridge-ready" });
await flush();
check("a disposed bridge answers nothing", replies.length === repliesBefore);

// --- The panel hosts the iframe at the gateway origin ----------------------------

{
  const panel = new GatewayConfigPanel({
    fetchOrigin: async () => GATEWAY_ORIGIN,
    workshopOrigin: WORKSHOP_ORIGIN,
  });
  panel.init({ params: {} });
  await flush();
  const iframe = panel.element.querySelector("iframe");
  check("the panel hosts an iframe once the origin resolves", iframe !== null);
  check(
    "the iframe loads the config SPA in panel mode with the bridge origin pinned",
    iframe?.getAttribute("src") ===
      `${GATEWAY_ORIGIN}/config/?mode=panel&bridge=${encodeURIComponent(WORKSHOP_ORIGIN)}`,
  );
  check(
    "the iframe sandbox grants scripts and same-origin only",
    iframe?.getAttribute("sandbox") === "allow-scripts allow-same-origin",
  );
  check("the iframe carries an accessible title", iframe?.getAttribute("title") === "Gateway Config");
  panel.dispose();
}

// --- The panel reports an unknown origin instead of loading ----------------------

{
  const panel = new GatewayConfigPanel({ fetchOrigin: async () => null });
  panel.init({ params: {} });
  await flush();
  const alert = panel.element.querySelector("[role='alert']");
  check(
    "an unknown gateway origin renders an alert, not an iframe",
    alert !== null && panel.element.querySelector("iframe") === null,
  );
  panel.dispose();
}

if (failures.length > 0) {
  console.error(`gateway-config-bridge: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("gateway-config-bridge: all assertions passed");
