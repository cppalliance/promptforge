// Unit test for the STT capability probe (src/ui/stt.ts
// sttCapability). Bundles the TS module with esbuild and drives it
// against scripted fetch responses: gpu/engine boolean combinations,
// non-OK status, network failure, and malformed bodies. The mic stays
// visible whatever the answer - the probe feeds the blocker reason the
// status bar names on click - so every failure mode must answer null.
// Run: node test/stt-capability.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "ui", "stt.ts")],
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
const { sttCapability } = mod;

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
    check("probe queries /stt/capability", url === "/stt/capability");
    return Promise.resolve(jsonResponse({ gpu: true, engine: true }));
  },
  async () => {
    const answer = await sttCapability();
    check(
      "gpu and engine true answer both true",
      answer !== null && answer.gpu === true && answer.engine === true,
    );
  },
);

await withFetch(
  () => Promise.resolve(jsonResponse({ gpu: false, engine: true })),
  async () => {
    const answer = await sttCapability();
    check(
      "gpu false answers gpu false with the engine flag intact",
      answer !== null && answer.gpu === false && answer.engine === true,
    );
  },
);

await withFetch(
  () => Promise.resolve(jsonResponse({ gpu: true, engine: false })),
  async () => {
    const answer = await sttCapability();
    check(
      "engine false answers engine false with the gpu flag intact",
      answer !== null && answer.gpu === true && answer.engine === false,
    );
  },
);

await withFetch(() => Promise.resolve(jsonResponse({ gpu: "yes", engine: true })), async () => {
  check("a non-boolean gpu answers null", (await sttCapability()) === null);
});

await withFetch(() => Promise.resolve(jsonResponse({ gpu: true })), async () => {
  check("a missing engine field answers null", (await sttCapability()) === null);
});

await withFetch(() => Promise.resolve(jsonResponse({})), async () => {
  check("a missing gpu field answers null", (await sttCapability()) === null);
});

await withFetch(() => Promise.resolve(jsonResponse("not json at all")), async () => {
  check("an unparseable body answers null", (await sttCapability()) === null);
});

await withFetch(() => Promise.resolve(jsonResponse({ gpu: true, engine: true }, 500)), async () => {
  check("a non-OK status answers null", (await sttCapability()) === null);
});

await withFetch(() => Promise.reject(new Error("connection refused")), async () => {
  check("a network failure answers null", (await sttCapability()) === null);
});

if (failures.length > 0) {
  console.error(`stt-capability test failed:\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("stt-capability test passed");
process.exit(0);
