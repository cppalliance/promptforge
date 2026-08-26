// Unit test for the GPU capability probe (src/voice.ts
// voiceGpuAvailable). Bundles the TS module with esbuild and drives it
// against scripted fetch responses: gpu true/false, non-OK status, network
// failure, and malformed bodies. The mic gate in main.ts hides the control
// unless the probe answers true, so every failure mode must answer false.
// Run: node test/voice-capability.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "voice.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  loader: { ".css": "empty" },
  logLevel: "silent",
});
const code = bundle.outputFiles[0].text;
const mod = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
const { voiceGpuAvailable } = mod;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function jsonResponse(body, status = 200) {
  return new Response(typeof body === "string" ? body : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function withFetch(impl, run) {
  const original = globalThis.fetch;
  globalThis.fetch = impl;
  try {
    await run();
  } finally {
    globalThis.fetch = original;
  }
}

await withFetch(
  (url) => {
    check("probe queries /voice/capability", url === "/voice/capability");
    return Promise.resolve(jsonResponse({ gpu: true }));
  },
  async () => {
    check("gpu true answers true", (await voiceGpuAvailable()) === true);
  },
);

await withFetch(() => Promise.resolve(jsonResponse({ gpu: false })), async () => {
  check("gpu false answers false", (await voiceGpuAvailable()) === false);
});

await withFetch(() => Promise.resolve(jsonResponse({ gpu: "yes" })), async () => {
  check("a non-boolean gpu answers false", (await voiceGpuAvailable()) === false);
});

await withFetch(() => Promise.resolve(jsonResponse({})), async () => {
  check("a missing gpu field answers false", (await voiceGpuAvailable()) === false);
});

await withFetch(() => Promise.resolve(jsonResponse("not json at all")), async () => {
  check("an unparseable body answers false", (await voiceGpuAvailable()) === false);
});

await withFetch(() => Promise.resolve(jsonResponse({ gpu: true }, 500)), async () => {
  check("a non-OK status answers false", (await voiceGpuAvailable()) === false);
});

await withFetch(() => Promise.reject(new Error("connection refused")), async () => {
  check("a network failure answers false", (await voiceGpuAvailable()) === false);
});

if (failures.length > 0) {
  console.error(`voice-capability test failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("voice-capability test passed");
process.exit(0);
