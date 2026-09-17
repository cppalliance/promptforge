// Unit test for the Run window's contract client (src/services/run-api.ts).
// Bundles the module with esbuild, imports it via a data URL, and drives it
// against a scripted fetch. Covers: a full contract narrows field by field
// (snake_case wire keys to the client's camelCase); the implicit-args shape;
// narrowing rejects an unknown tool kind and an unknown arg type; a 422
// envelope surfaces the server's code and message; a non-JSON answer and a
// transport failure throw their typed variants.
// Run: node --test test/run-api.mjs
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "services", "run-api.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});
const { fetchPromptContract } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The full-frontmatter contract, as POST /prompts/contract answers it.
const CONTRACT = {
  name: "papergate",
  description: "Screens papers",
  promptforge: 1,
  max_tool_iterations: 12,
  input: { path: "in/papers.md", description: "the papers" },
  output: { path: "out/verdicts.md", description: "the verdicts" },
  capabilities: [{ id: "tools/web", optional: false }],
  tools: [
    { kind: "exact", alias: "search", path: "tools/web/search" },
    { kind: "fuzzy", alias: "fetch", want: "fetch a page", optional: true },
  ],
  args: {
    implicit: false,
    fields: [
      { name: "topic", type: "string", optional: false, default: null, description: "the topic" },
      { name: "deep", type: "boolean", optional: true, default: false, description: null },
      { name: "limit", type: "integer", optional: true, default: 5, description: null },
    ],
  },
  models: [
    { label: "main", keywords: ["thinking", "frontier"], min_context: 32000, description: null },
  ],
};

function scriptFetch(responder) {
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url, init });
    return responder(url, init);
  };
  return calls;
}

const json = (body, status = 200) => ({
  ok: status >= 200 && status < 300,
  status,
  json: async () => body,
});

// --- A full contract narrows field by field --------------------------------

{
  const calls = scriptFetch(() => json(CONTRACT));
  const contract = await fetchPromptContract("papergate.md", "prompt text");
  check(
    "the request posts name and text to the contract route",
    calls.length === 1 &&
      calls[0].url === "/prompts/contract" &&
      calls[0].init.method === "POST" &&
      JSON.parse(calls[0].init.body).name === "papergate.md" &&
      JSON.parse(calls[0].init.body).text === "prompt text",
  );
  check(
    "scalars narrow",
    contract.name === "papergate" &&
      contract.description === "Screens papers" &&
      contract.promptforge === 1 &&
      contract.maxToolIterations === 12,
  );
  check(
    "input and output narrow",
    contract.input?.path === "in/papers.md" &&
      contract.output?.path === "out/verdicts.md" &&
      contract.output?.description === "the verdicts",
  );
  check(
    "capabilities narrow",
    contract.capabilities.length === 1 &&
      contract.capabilities[0].id === "tools/web" &&
      contract.capabilities[0].optional === false,
  );
  check(
    "tool kinds narrow into their tagged variants",
    contract.tools.length === 2 &&
      contract.tools[0].kind === "exact" &&
      contract.tools[0].path === "tools/web/search" &&
      contract.tools[1].kind === "fuzzy" &&
      contract.tools[1].want === "fetch a page" &&
      contract.tools[1].optional === true,
  );
  check(
    "arg fields narrow with defaults and descriptions",
    contract.args.implicit === false &&
      contract.args.fields.length === 3 &&
      contract.args.fields[0].name === "topic" &&
      contract.args.fields[0].type === "string" &&
      contract.args.fields[0].optional === false &&
      contract.args.fields[1].default === false &&
      contract.args.fields[2].type === "integer" &&
      contract.args.fields[2].default === 5,
  );
  check(
    "model roles narrow",
    contract.models.length === 1 &&
      contract.models[0].label === "main" &&
      contract.models[0].keywords.join(",") === "thinking,frontier" &&
      contract.models[0].minContext === 32000,
  );
}

// --- Absent sections narrow to null; implicit args carry the prose field ----

{
  scriptFetch(() =>
    json({
      name: "bare",
      description: "",
      promptforge: null,
      max_tool_iterations: null,
      input: null,
      output: null,
      capabilities: [],
      tools: [],
      args: {
        implicit: true,
        fields: [
          { name: "prose", type: "string", optional: false, default: null, description: null },
        ],
      },
      models: [],
    }),
  );
  const contract = await fetchPromptContract("bare.md", "text");
  check(
    "absent sections narrow to null",
    contract.promptforge === null &&
      contract.maxToolIterations === null &&
      contract.input === null &&
      contract.output === null,
  );
  check(
    "the implicit args declaration carries the single prose field",
    contract.args.implicit === true &&
      contract.args.fields.length === 1 &&
      contract.args.fields[0].name === "prose" &&
      contract.args.fields[0].type === "string",
  );
}

// --- Narrowing rejects unknown variants -------------------------------------

{
  scriptFetch(() =>
    json({
      ...CONTRACT,
      tools: [{ kind: "open", alias: "x", path: "tools/web/search" }],
    }),
  );
  let caught = null;
  try {
    await fetchPromptContract("p.md", "text");
  } catch (error) {
    caught = error;
  }
  check(
    "an unknown tool kind is a shape failure",
    caught !== null && caught.code === "unexpected_shape",
  );
}

{
  scriptFetch(() =>
    json({
      ...CONTRACT,
      args: {
        implicit: false,
        fields: [
          { name: "when", type: "date", optional: true, default: null, description: null },
        ],
      },
    }),
  );
  let caught = null;
  try {
    await fetchPromptContract("p.md", "text");
  } catch (error) {
    caught = error;
  }
  check(
    "an unknown arg type is a shape failure",
    caught !== null && caught.code === "unexpected_shape",
  );
}

// --- A 422 envelope surfaces the server's code and message ------------------

{
  scriptFetch(() =>
    json(
      { error: { code: "parse_frontmatter", message: "line 3: bad YAML key" } },
      422,
    ),
  );
  let caught = null;
  try {
    await fetchPromptContract("broken.md", "text");
  } catch (error) {
    caught = error;
  }
  check(
    "a 422 surfaces the status",
    caught !== null && caught.status === 422,
  );
  check(
    "a 422 surfaces the server's code and message",
    caught !== null &&
      caught.message.includes("parse_frontmatter") &&
      caught.message.includes("line 3: bad YAML key"),
  );
}

// --- Transport and non-JSON failures throw their typed variants -------------

{
  scriptFetch(() => {
    throw new TypeError("connection refused");
  });
  let caught = null;
  try {
    await fetchPromptContract("p.md", "text");
  } catch (error) {
    caught = error;
  }
  check("a transport failure is typed", caught !== null && caught.code === "transport");
}

if (failures.length > 0) {
  console.error(`run-api: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("run-api: all assertions passed");
process.exit(0);
