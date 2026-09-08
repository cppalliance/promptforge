import assert from "node:assert/strict";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test, { after, before } from "node:test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const validator = join(root, "tools", "validate-rust-1.89.0.ps1");
const fixtureRoot = mkdtempSync(join(tmpdir(), "promptforge-rust-contract-"));
const fakeToolSource = join(fixtureRoot, "fake-rust-tool.rs");
const fakeTool = join(
  fixtureRoot,
  process.platform === "win32" ? "fake-rust-tool.exe" : "fake-rust-tool",
);

function environmentKey(environment, name) {
  return Object.keys(environment).find(
    (key) => key.toLowerCase() === name.toLowerCase(),
  );
}

function setEnvironmentVariable(environment, name, value) {
  const key = environmentKey(environment, name) ?? name;
  environment[key] = value;
}

function deleteEnvironmentVariable(environment, name) {
  const key = environmentKey(environment, name);
  if (key) {
    delete environment[key];
  }
}

function createToolLayout(names = ["cargo", "rustc"]) {
  const bin = mkdtempSync(join(fixtureRoot, "bin-"));
  for (const name of names) {
    const destination = join(bin, `${name}.exe`);
    copyFileSync(fakeTool, destination);
    chmodSync(destination, 0o755);
  }
  return bin;
}

function runValidator({
  bin,
  cargoVersion = "1.89.0",
  contract = bin,
  rustcVersion = "1.89.0",
} = {}) {
  const environment = { ...process.env };
  const runRoot = mkdtempSync(join(fixtureRoot, "run-"));
  const githubPath = join(runRoot, "github-path");
  const invocationLog = join(runRoot, "invocations");

  deleteEnvironmentVariable(environment, "PROMPTFORGE_RUST_1_89_0_BIN");
  if (contract !== undefined) {
    setEnvironmentVariable(
      environment,
      "PROMPTFORGE_RUST_1_89_0_BIN",
      contract,
    );
  }
  setEnvironmentVariable(environment, "GITHUB_PATH", githubPath);
  setEnvironmentVariable(environment, "FAKE_INVOCATION_LOG", invocationLog);
  setEnvironmentVariable(environment, "FAKE_CARGO_VERSION", cargoVersion);
  setEnvironmentVariable(environment, "FAKE_RUSTC_VERSION", rustcVersion);

  const powershell = process.platform === "win32"
    ? join(
        process.env.SystemRoot ?? "C:\\WINDOWS",
        "System32",
        "WindowsPowerShell",
        "v1.0",
        "powershell.exe",
      )
    : "pwsh";
  const result = spawnSync(
    powershell,
    ["-NoProfile", "-NonInteractive", "-File", validator],
    {
      cwd: root,
      encoding: "utf8",
      env: environment,
      timeout: 30_000,
    },
  );
  return {
    ...result,
    githubPath,
    invocationLog,
    output: `${result.stdout ?? ""}${result.stderr ?? ""}`.replace(/\s+/g, " "),
  };
}

before(() => {
  writeFileSync(
    fakeToolSource,
    String.raw`use std::env;
use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    let name = env::current_exe()
        .expect("current executable")
        .file_stem()
        .expect("executable stem")
        .to_string_lossy()
        .to_ascii_lowercase();
    let arguments: Vec<_> = env::args().skip(1).collect();
    let mut log = OpenOptions::new()
        .append(true)
        .create(true)
        .open(env::var("FAKE_INVOCATION_LOG").expect("invocation log"))
        .expect("open invocation log");
    writeln!(log, "{name} {}", arguments.join(" ")).expect("write invocation log");
    let version = match name.as_str() {
        "cargo" => env::var("FAKE_CARGO_VERSION").expect("cargo version"),
        "rustc" => env::var("FAKE_RUSTC_VERSION").expect("rustc version"),
        _ => panic!("unexpected fake tool name: {name}"),
    };
    println!("{name} {version} (fixture 2026-09-08)");
}
`,
  );
  const compiled = spawnSync("rustc", [fakeToolSource, "-o", fakeTool], {
    cwd: root,
    encoding: "utf8",
    timeout: 30_000,
  });
  assert.equal(
    compiled.status,
    0,
    `failed to compile fake Rust tools:\n${compiled.stdout}${compiled.stderr}`,
  );
});

after(() => {
  rmSync(fixtureRoot, { force: true, recursive: true });
});

test("accepts valid direct Rust tools", () => {
  const bin = createToolLayout();
  const result = runValidator({ bin });

  assert.equal(result.status, 0, result.output);
  assert.match(result.stdout, /Validated cargo 1\.89\.0 from /);
  assert.match(result.stdout, /Validated rustc 1\.89\.0 from /);
  assert.equal(readFileSync(result.githubPath, "utf8").trim(), resolve(bin));
});

test("never passes a toolchain selector to either executable", () => {
  const bin = createToolLayout();
  const result = runValidator({ bin });

  assert.equal(result.status, 0, result.output);
  assert.deepEqual(
    readFileSync(result.invocationLog, "utf8").trim().split(/\r?\n/),
    ["cargo --version", "rustc --version"],
  );
});

test("rejects an unset contract instead of discovering Rust", () => {
  const result = runValidator({ contract: undefined });

  assert.notEqual(result.status, 0);
  assert.match(result.output, /PROMPTFORGE_RUST_1_89_0_BIN must be set/);
});

test("rejects relative and nonexistent contract directories", () => {
  for (const [contract, message] of [
    [join("relative", "rust-bin"), /must be an absolute directory/],
    [join(fixtureRoot, "does-not-exist"), /directory does not exist/],
  ]) {
    const result = runValidator({ contract });
    assert.notEqual(result.status, 0);
    assert.match(result.output, message);
  }
});

test("rejects missing or non-file executables", () => {
  const missingRustc = createToolLayout(["cargo"]);
  const nonFileCargo = createToolLayout(["rustc"]);
  mkdirSync(join(nonFileCargo, "cargo.exe"));

  for (const bin of [missingRustc, nonFileCargo]) {
    const result = runValidator({ bin });
    assert.notEqual(result.status, 0);
    assert.match(result.output, /must contain regular cargo\.exe and rustc\.exe files/);
  }
});

test("rejects malformed tool version output", () => {
  for (const versions of [
    { cargoVersion: "stable" },
    { rustcVersion: "unknown" },
  ]) {
    const result = runValidator({ bin: createToolLayout(), ...versions });
    assert.notEqual(result.status, 0);
    assert.match(result.output, /returned malformed version output/);
  }
});

test("rejects every wrong or mixed Rust version", () => {
  for (const versions of [
    { cargoVersion: "1.90.0" },
    { rustcVersion: "1.88.0" },
    { cargoVersion: "1.89.0", rustcVersion: "1.90.0" },
  ]) {
    const result = runValidator({ bin: createToolLayout(), ...versions });
    assert.notEqual(result.status, 0);
    assert.match(result.output, /requires exactly 1\.89\.0/);
  }
});
