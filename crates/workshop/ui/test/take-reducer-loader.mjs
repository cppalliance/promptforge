// The take reducer bundled from its TypeScript sources, and the editor edit
// applier, shared by the tests that render server events into editor text.

import assert from "node:assert/strict";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export {
        createTakeRegistry,
        reduceTakeRegistry,
      } from "./src/parts/take/take-registry.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
});

export const { createTakeRegistry, reduceTakeRegistry } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);

export function applyEdits(text, effects) {
  let next = text;
  for (const effect of effects) {
    if (effect.domain !== "editor" || effect.command !== "replace") {
      continue;
    }
    assert.ok(
      effect.from >= 0 && effect.from <= effect.to && effect.to <= next.length,
      `replace [${effect.from}, ${effect.to}] lies inside ${JSON.stringify(next)}`,
    );
    next = next.slice(0, effect.from) + effect.text + next.slice(effect.to);
  }
  return next;
}
