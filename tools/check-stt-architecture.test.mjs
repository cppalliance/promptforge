import assert from "node:assert/strict";
import { join } from "node:path";
import test from "node:test";

import {
  assertAcyclic,
  countEffectiveRootNames,
  parseCargoModulesDot,
  publicRootCount,
  requireCargoVersion,
  requireExactPublicRootCount,
  requireToolVersion,
  runCargo,
  runPublicApi,
  runRustdocCargo,
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

test("public root count is exact rather than a spare budget", () => {
  assert.equal(publicRootCount("public_root_count = 6\n", "demo"), 6);
  assert.doesNotThrow(() => requireExactPublicRootCount("demo", 6, 6));
  assert.throws(
    () => requireExactPublicRootCount("demo", 5, 6),
    /expected exactly 6/,
  );
  assert.throws(
    () => requireExactPublicRootCount("demo", 7, 6),
    /expected exactly 6/,
  );
});

test("tool version parser rejects an unpinned version", () => {
  assert.throws(
    () => requireToolVersion("cargo-modules", "cargo-modules 0.26.0\n", "0.25.0"),
    /requires cargo-modules 0\.25\.0/,
  );
});

test("Cargo version parser rejects ambient Cargo 1.98", () => {
  assert.throws(
    () => requireCargoVersion("cargo 1.98.0 (797e8a9bc 2026-08-05)\n"),
    /requires Cargo 1\.89\.0/,
  );
});

test("cargo-modules child cannot inherit ambient Cargo 1.98", () => {
  let child;
  runCargo("repo", ["modules", "--version"], {
    env: { AMBIENT_CARGO_VERSION: "1.98.0", PATH: "rustup", RUSTUP_TOOLCHAIN: "stable" },
    spawn(command, args, options) {
      child = { command, args, options };
      return { status: 0, stdout: "cargo-modules 0.25.0\n", stderr: "" };
    },
  });

  assert.equal(child.command, "cargo");
  assert.deepEqual(child.args, ["modules", "--version"]);
  assert.equal(child.options.env.AMBIENT_CARGO_VERSION, "1.98.0");
  assert.equal(child.options.env.PATH, "rustup");
  assert.equal(child.options.env.RUSTUP_TOOLCHAIN, "1.89.0");
});

test("cargo-public-api child cannot inherit ambient Cargo 1.98", () => {
  let child;
  runRustdocCargo("repo", ["public-api", "--version"], {
    env: { AMBIENT_CARGO_VERSION: "1.98.0", PATH: "rustup", RUSTUP_TOOLCHAIN: "stable" },
    spawn(command, args, options) {
      child = { command, args, options };
      return { status: 0, stdout: "cargo-public-api 0.52.0\n", stderr: "" };
    },
  });

  assert.equal(child.command, "cargo");
  assert.deepEqual(child.args, [
    "+nightly-2026-09-05",
    "public-api",
    "--version",
  ]);
  assert.equal(child.options.env.AMBIENT_CARGO_VERSION, "1.98.0");
  assert.equal(child.options.env.PATH, "rustup");
  assert.equal(child.options.env.RUSTUP_TOOLCHAIN, "nightly-2026-09-05");
});

test("public API command keeps exact package selection", () => {
  let child;
  runPublicApi("repo", "gateway-stt", {
    env: { RUSTUP_TOOLCHAIN: "stable" },
    spawn(command, args, options) {
      child = { command, args, options };
      return { status: 0, stdout: "pub mod gateway_stt\n", stderr: "" };
    },
  });

  assert.equal(child.command, "cargo");
  assert.deepEqual(child.args, [
    "+nightly-2026-09-05",
    "public-api",
    "--manifest-path",
    join("repo", "crates", "gateway-stt", "Cargo.toml"),
    "--package",
    "gateway-stt",
    "-sss",
    "--color",
    "never",
  ]);
  assert.equal(child.options.env.RUSTUP_TOOLCHAIN, "nightly-2026-09-05");
});

test("public API command fails closed when the pinned nightly is absent", () => {
  assert.throws(
    () =>
      runPublicApi("repo", "gateway-stt", {
        spawn(command, args) {
          assert.equal(command, "cargo");
          assert.equal(args[0], "+nightly-2026-09-05");
          return {
            status: 1,
            stdout: "",
            stderr:
              "error: toolchain 'nightly-2026-09-05-x86_64-unknown-linux-gnu' is not installed",
          };
        },
      }),
    /cargo \+nightly-2026-09-05 public-api.*failed with status 1.*toolchain 'nightly-2026-09-05-x86_64-unknown-linux-gnu' is not installed/s,
  );
});

test("public API failure never falls back to the virtual workspace manifest", () => {
  const calls = [];
  assert.throws(
    () =>
      runPublicApi("repo", "gateway-stt", {
        spawn(command, args) {
          calls.push({ command, args });
          return {
            status: 1,
            stdout: "",
            stderr:
              "`Cargo.toml` is a virtual manifest; workspace API listing is unsupported",
          };
        },
      }),
    /failed with status 1.*virtual manifest/s,
  );

  assert.equal(calls.length, 1);
  const manifestIndex = calls[0].args.indexOf("--manifest-path");
  assert.equal(
    calls[0].args[manifestIndex + 1],
    join("repo", "crates", "gateway-stt", "Cargo.toml"),
  );
  assert.notEqual(calls[0].args[manifestIndex + 1], join("repo", "Cargo.toml"));
});

test("architecture cargo fails closed when Rust 1.89 is absent", () => {
  assert.throws(
    () =>
      runCargo("repo", ["--version"], {
        spawn() {
          return {
            status: 1,
            stdout: "",
            stderr: "toolchain '1.89.0' is not installed",
          };
        },
      }),
    /failed with status 1.*toolchain '1\.89\.0' is not installed/s,
  );
});

test("architecture cargo fails closed when a required tool is absent", () => {
  assert.throws(
    () =>
      runCargo("repo", ["modules", "--version"], {
        spawn() {
          return {
            status: 101,
            stdout: "",
            stderr: "no such command: `modules`",
          };
        },
      }),
    /failed with status 101.*no such command: `modules`/s,
  );
});
