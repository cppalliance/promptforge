import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const CARGO_MODULES_VERSION = "0.25.0";
const CARGO_PUBLIC_API_VERSION = "0.52.0";
const CARGO_VERSION = "1.89.0";
const CARGO_TOOLCHAIN = "1.89.0";
const RUSTDOC_TOOLCHAIN = "nightly-2026-09-05";
const STT_CRATES = [
  "gateway-stt",
  "gateway-stt-engine",
  "gateway-stt-backend-whisper",
  "gateway-whisper-ffi",
];
const FIXTURE_STT_CRATES = new Set(["gateway-stt", "gateway-stt-engine"]);

function fail(message) {
  throw new Error(message);
}

export function requireToolVersion(tool, output, expected) {
  const actual = output.trim();
  if (actual !== `${tool} ${expected}`) {
    fail(`architecture gate requires ${tool} ${expected}, got ${JSON.stringify(actual)}`);
  }
}

export function requireCargoVersion(output) {
  const actual = output.trim();
  if (!actual.startsWith(`cargo ${CARGO_VERSION} `)) {
    fail(`architecture gate requires Cargo ${CARGO_VERSION}, got ${JSON.stringify(actual)}`);
  }
}

function moduleOwner(item, modules) {
  return modules.find(
    (module) => item === module || item.startsWith(`${module}::`),
  );
}

export function parseCargoModulesDot(output) {
  const nodes = new Set();
  const crateNodes = [];
  const rawEdges = [];
  const value = String.raw`(?:"[^"\\]*"|[A-Za-z_][A-Za-z0-9_]*|\d+(?:\.\d+)?)`;
  const attributes = String.raw`\[(?:[A-Za-z_][A-Za-z0-9_]*=${value})(?:,\s*[A-Za-z_][A-Za-z0-9_]*=${value})*\]`;
  const nodePattern = new RegExp(
    String.raw`^\s*"([^"\\]+)"\s+${attributes};\s*// "(crate|mod)" node\s*$`,
  );
  const edgePattern = new RegExp(
    String.raw`^\s*"([^"\\]+)"\s+->\s+"([^"\\]+)"(?:\s+${attributes})+;\s*// "uses" edge\s*$`,
  );
  const attributePattern = new RegExp(
    String.raw`^[A-Za-z_][A-Za-z0-9_]*\s*=\s*${value},\s*$`,
  );
  let sawDigraph = false;
  let sawClose = false;
  let attributeBlock;

  for (const line of output.split(/\r?\n/)) {
    const statement = line.trim();
    if (statement.length === 0) {
      continue;
    }
    if (!sawDigraph && statement === "digraph {") {
      sawDigraph = true;
      continue;
    }
    if (!sawDigraph || sawClose) {
      fail(`malformed cargo-modules DOT statement: ${statement}`);
    }
    if (attributeBlock !== undefined) {
      if (statement === "];") {
        attributeBlock = undefined;
      } else if (
        !statement.startsWith("//") &&
        !attributePattern.test(statement)
      ) {
        fail(`malformed cargo-modules DOT ${attributeBlock} attribute: ${statement}`);
      }
      continue;
    }
    if (statement === "}") {
      sawClose = true;
      continue;
    }
    const block = /^(graph|node|edge) \[$/.exec(statement);
    if (block !== null) {
      attributeBlock = block[1];
      continue;
    }
    if (line.includes('// "crate" node') || line.includes('// "mod" node')) {
      const match = nodePattern.exec(line);
      if (match === null) {
        fail(`malformed cargo-modules DOT node: ${line.trim()}`);
      }
      if (nodes.has(match[1])) {
        fail(`malformed cargo-modules DOT output: duplicate node ${match[1]}`);
      }
      nodes.add(match[1]);
      if (match[2] === "crate") {
        crateNodes.push(match[1]);
      }
      continue;
    }
    if (line.includes('// "uses" edge')) {
      const match = edgePattern.exec(line);
      if (match === null) {
        fail(`malformed cargo-modules DOT edge: ${line.trim()}`);
      }
      rawEdges.push([match[1], match[2]]);
      continue;
    }
    fail(`malformed cargo-modules DOT statement: ${statement}`);
  }

  if (!sawDigraph || !sawClose || attributeBlock !== undefined) {
    fail("malformed cargo-modules DOT output: expected one complete digraph");
  }

  if (crateNodes.length !== 1) {
    fail(
      `malformed cargo-modules DOT output: expected one crate node, got ${crateNodes.length}`,
    );
  }
  const crate = crateNodes[0];
  for (const node of nodes) {
    if (node !== crate && !node.startsWith(`${crate}::`)) {
      fail(`malformed cargo-modules DOT output: node ${node} is outside ${crate}`);
    }
  }

  const modules = [...nodes].sort((left, right) => right.length - left.length);
  const graph = new Map(modules.map((module) => [module, new Set()]));
  for (const [rawSource, rawTarget] of rawEdges) {
    const source = moduleOwner(rawSource, modules);
    const target = moduleOwner(rawTarget, modules);
    if (source === undefined || target === undefined) {
      const item = source === undefined ? rawSource : rawTarget;
      fail(
        `malformed cargo-modules DOT output: ${item} does not belong to a declared module`,
      );
    }
    if (source !== target) {
      graph.get(source).add(target);
    }
  }
  return graph;
}

export function assertAcyclic(graph, graphName) {
  const state = new Map();
  const stack = [];

  function visit(node) {
    if (state.get(node) === 2) {
      return;
    }
    if (state.get(node) === 1) {
      const start = stack.indexOf(node);
      fail(`${graphName} module cycle: ${[...stack.slice(start), node].join(" -> ")}`);
    }
    if (!graph.has(node)) {
      fail(`${graphName} graph references unknown module ${node}`);
    }
    state.set(node, 1);
    stack.push(node);
    for (const target of graph.get(node)) {
      visit(target);
    }
    stack.pop();
    state.set(node, 2);
  }

  for (const node of graph.keys()) {
    visit(node);
  }
}

function escapedRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function countEffectiveRootNames(output, crateName) {
  const lines = output.split(/\r?\n/).filter((line) => line.length > 0);
  if (lines[0] !== `pub mod ${crateName}`) {
    fail(
      `malformed cargo-public-api output for ${crateName}: missing crate root declaration`,
    );
  }

  const pathPattern = new RegExp(
    `\\b${escapedRegExp(crateName)}::(r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)`,
    "g",
  );
  const names = new Set();
  for (const line of lines.slice(1)) {
    if (!/^(?:#\[[^\]]+\]\s+)?(?:pub|impl)\b/.test(line)) {
      fail(`malformed cargo-public-api output for ${crateName}: ${line}`);
    }
    const matches = [...line.matchAll(pathPattern)];
    if (matches.length === 0) {
      fail(
        `malformed cargo-public-api output for ${crateName}: item has no crate path: ${line}`,
      );
    }
    for (const match of matches) {
      names.add(match[1]);
    }
  }
  return names.size;
}

export function publicRootCount(source, crateName) {
  const matches = [
    ...source.matchAll(/^\s*public_root_count\s*=\s*(\d+)\s*$/gm),
  ];
  if (matches.length !== 1) {
    fail(
      `${crateName}/module-ceilings.toml must contain exactly one integer public_root_count`,
    );
  }
  return Number(matches[0][1]);
}

export function testFixturePublicRootCount(source, crateName) {
  const matches = [
    ...source.matchAll(
      /^\s*test_fixture_public_root_count\s*=\s*(\d+)\s*$/gm,
    ),
  ];
  if (matches.length !== 1) {
    fail(
      `${crateName}/module-ceilings.toml must contain exactly one integer test_fixture_public_root_count`,
    );
  }
  return Number(matches[0][1]);
}

export function requireExactPublicRootCount(crateName, actual, expected) {
  if (actual !== expected) {
    fail(
      `${crateName} exposes ${actual} effective root names, expected exactly ${expected}`,
    );
  }
}

function canonicalPublicApi(output, crateName) {
  const canonical = output.replaceAll("\r\n", "\n");
  if (canonical.includes("\r") || !canonical.endsWith("\n")) {
    fail(
      `malformed cargo-public-api output for ${crateName}: expected LF-terminated lines`,
    );
  }
  countEffectiveRootNames(canonical, crateName);
  return canonical;
}

export function requireExactPublicApi(crateName, actual, expected) {
  const actualCanonical = canonicalPublicApi(actual, crateName);
  const expectedCanonical = canonicalPublicApi(expected, crateName);
  if (actualCanonical !== expectedCanonical) {
    fail(
      `${crateName} feature-enabled public API differs from its exact snapshot`,
    );
  }
}

export function requireNoFixtureApi(crateName, output, expected) {
  const canonical = canonicalPublicApi(output, crateName);
  const expectedCanonical = canonicalPublicApi(expected, crateName);
  if (canonical !== expectedCanonical) {
    fail(`${crateName} default build exposes fixture API`);
  }
}

export function runCargo(
  root,
  args,
  { spawn = spawnSync, env = process.env } = {},
) {
  const result = spawn("cargo", args, {
    cwd: root,
    encoding: "utf8",
    env: { ...env, RUSTUP_TOOLCHAIN: CARGO_TOOLCHAIN },
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error !== undefined) {
    fail(`cargo ${args[0]} failed to start: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(
      `cargo ${args.join(" ")} failed with status ${result.status}\n${result.stderr}`,
    );
  }
  return result.stdout;
}

export function runRustdocCargo(
  root,
  args,
  { spawn = spawnSync, env = process.env } = {},
) {
  const commandArgs = [`+${RUSTDOC_TOOLCHAIN}`, ...args];
  const result = spawn("cargo", commandArgs, {
    cwd: root,
    encoding: "utf8",
    env: { ...env, RUSTUP_TOOLCHAIN: RUSTDOC_TOOLCHAIN },
    maxBuffer: 64 * 1024 * 1024,
    windowsHide: true,
  });
  if (result.error !== undefined) {
    fail(`cargo ${commandArgs.join(" ")} failed to start: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(
      `cargo ${commandArgs.join(" ")} failed with status ${result.status}\n${result.stderr}`,
    );
  }
  return result.stdout;
}

export function runPublicApi(
  root,
  crateName,
  { features = [], spawn = spawnSync, env = process.env } = {},
) {
  const manifestPath = join(root, "crates", crateName, "Cargo.toml");
  const featureArgs =
    features.length === 0 ? [] : ["--features", features.join(",")];
  return runRustdocCargo(
    root,
    [
      "public-api",
      "--manifest-path",
      manifestPath,
      "--package",
      crateName,
      ...featureArgs,
      "-sss",
      "--color",
      "never",
    ],
    { spawn, env },
  );
}

function checkNodeVersion() {
  const major = Number(process.versions.node.split(".")[0]);
  if (!Number.isInteger(major) || major < 22) {
    fail(`architecture gate requires Node.js 22 or later, got ${process.versions.node}`);
  }
}

function main() {
  checkNodeVersion();
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  requireCargoVersion(runCargo(root, ["--version"]));
  requireToolVersion(
    "cargo-modules",
    runCargo(root, ["modules", "--version"]),
    CARGO_MODULES_VERSION,
  );
  requireToolVersion(
    "cargo-public-api",
    runRustdocCargo(root, ["public-api", "--version"]),
    CARGO_PUBLIC_API_VERSION,
  );

  for (const crateName of STT_CRATES) {
    const dot = runCargo(root, [
      "modules",
      "dependencies",
      "--lib",
      "-p",
      crateName,
      "--no-externs",
      "--no-fns",
      "--no-sysroot",
      "--no-traits",
      "--no-types",
      "--no-owns",
      "--layout",
      "dot",
    ]);
    assertAcyclic(parseCargoModulesDot(dot), crateName);

    const publicApi = runPublicApi(root, crateName);
    const rootNames = countEffectiveRootNames(
      publicApi,
      crateName.replaceAll("-", "_"),
    );
    const ceilingPath = join(root, "crates", crateName, "module-ceilings.toml");
    const expected = publicRootCount(readFileSync(ceilingPath, "utf8"), crateName);
    requireExactPublicRootCount(crateName, rootNames, expected);
    console.log(`${crateName}: acyclic, public roots ${rootNames}`);

    if (FIXTURE_STT_CRATES.has(crateName)) {
      const defaultSnapshotPath = join(
        root,
        "crates",
        crateName,
        "public-api-default.txt",
      );
      requireNoFixtureApi(
        crateName.replaceAll("-", "_"),
        publicApi,
        readFileSync(defaultSnapshotPath, "utf8"),
      );
      const fixturePublicApi = runPublicApi(root, crateName, {
        features: ["test-fixtures"],
      });
      const snapshotPath = join(
        root,
        "crates",
        crateName,
        "public-api-test-fixtures.txt",
      );
      requireExactPublicApi(
        crateName.replaceAll("-", "_"),
        fixturePublicApi,
        readFileSync(snapshotPath, "utf8"),
      );
      const fixtureRootNames = countEffectiveRootNames(
        fixturePublicApi,
        crateName.replaceAll("-", "_"),
      );
      const expectedFixtureRoots = testFixturePublicRootCount(
        readFileSync(ceilingPath, "utf8"),
        crateName,
      );
      requireExactPublicRootCount(
        `${crateName} test-fixtures`,
        fixtureRootNames,
        expectedFixtureRoots,
      );
      console.log(
        `${crateName}: test-fixtures API exact, public roots ${fixtureRootNames}`,
      );
    }
  }
}

const invokedPath =
  process.argv[1] === undefined
    ? undefined
    : pathToFileURL(resolve(process.argv[1])).href;
if (invokedPath === import.meta.url) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
