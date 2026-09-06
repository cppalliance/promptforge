import assert from "node:assert/strict";
import test from "node:test";

import {
  assertAcyclic,
  countEffectiveRootNames,
  parseCargoModulesDot,
  requireToolVersion,
} from "./check-stt-architecture.mjs";

test("DOT parser collapses item edges to their owning modules", () => {
  const graph = parseCargoModulesDot(`
digraph {
  "demo" [label="crate|demo"]; // "crate" node
  "demo::alpha" [label="mod|alpha"]; // "mod" node
  "demo::beta" [label="mod|beta"]; // "mod" node
  "demo::alpha::Thing" -> "demo::beta::Other" [label="uses"]; // "uses" edge
}
`);

  assert.deepEqual([...graph.get("demo::alpha")], ["demo::beta"]);
  assert.doesNotThrow(() => assertAcyclic(graph, "demo"));
});

test("cycle checker catches collapsed module cycles", () => {
  const graph = parseCargoModulesDot(`
digraph {
  "demo" [label="crate|demo"]; // "crate" node
  "demo::alpha" [label="mod|alpha"]; // "mod" node
  "demo::beta" [label="mod|beta"]; // "mod" node
  "demo::alpha::Thing" -> "demo::beta::Other" [label="uses"]; // "uses" edge
  "demo::beta::Other" -> "demo::alpha::Thing" [label="uses"]; // "uses" edge
}
`);

  assert.throws(() => assertAcyclic(graph, "demo"), /alpha.*beta.*alpha/);
});

test("DOT parser rejects edges outside declared module nodes", () => {
  assert.throws(
    () =>
      parseCargoModulesDot(`
digraph {
  "demo" [label="crate|demo"]; // "crate" node
  "demo::alpha" [label="mod|alpha"]; // "mod" node
  "demo::alpha::Thing" -> "other::Thing" [label="uses"]; // "uses" edge
}
`),
    /does not belong to a declared module/,
  );
});

test("DOT parser rejects a malformed edge that would complete a cycle", () => {
  assert.throws(
    () =>
      parseCargoModulesDot(`
digraph {
  "demo" [label="crate|demo"]; // "crate" node
  "demo::alpha" [label="mod|alpha"]; // "mod" node
  "demo::beta" [label="mod|beta"]; // "mod" node
  "demo::alpha::Thing" -> "demo::beta::Other" [label="uses"]; // "uses" edge
  "demo::beta::Other" -> BROKEN
}
`),
    /malformed cargo-modules DOT statement/,
  );
});

test("public API parser counts unique effective root names", () => {
  const count = countEffectiveRootNames(
    `
pub mod demo
pub struct demo::One
impl demo::One
pub fn demo::One::new() -> Self
pub type demo::Alias = demo::One
`,
    "demo",
  );

  assert.equal(count, 2);
});

test("public API parser rejects malformed output", () => {
  assert.throws(
    () => countEffectiveRootNames("pub mod demo\nnot public API output\n", "demo"),
    /malformed cargo-public-api output/,
  );
});

test("tool version parser rejects an unpinned version", () => {
  assert.throws(
    () => requireToolVersion("cargo-modules", "cargo-modules 0.26.0\n", "0.25.0"),
    /requires cargo-modules 0\.25\.0/,
  );
});
