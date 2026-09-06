import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(
  join(root, ".github", "workflows", "stt-miri.yml"),
  "utf8",
);

function jobSource(name) {
  const marker = `  ${name}:\n`;
  const start = workflow.indexOf(marker);
  assert.notEqual(start, -1, `missing ${name} job`);
  const remainder = workflow.slice(start + marker.length);
  const nextJob = remainder.match(/^  [A-Za-z0-9_-]+:\r?$/m);
  const end = nextJob
    ? start + marker.length + nextJob.index
    : workflow.length;
  return workflow.slice(start, end);
}

test("native runner validates provisioned stable Rust before caching", () => {
  const native = jobSource("native-whisper");
  const preflight = native.indexOf("- name: Verify preinstalled stable Rust");
  const cache = native.indexOf("- name: Cache Cargo");
  const disableAutoInstall = native.indexOf(
    "$env:RUSTUP_AUTO_INSTALL = '0'",
  );
  const listToolchains = native.indexOf(
    "$toolchains = @(& $rustup toolchain list)",
  );
  const requireStable = native.indexOf("if (-not $stableToolchain)");
  const invokeCargo = native.indexOf("& $cargo '+stable' '--version'");

  assert.ok(preflight > 0, "native job must have a Rust preflight");
  assert.ok(cache > preflight, "Rust preflight must run before Cargo caching");
  assert.ok(
    disableAutoInstall > preflight,
    "native preflight must disable rustup auto-install",
  );
  assert.ok(
    listToolchains > disableAutoInstall,
    "native preflight must inspect installed toolchains after disabling auto-install",
  );
  assert.ok(
    requireStable > listToolchains,
    "native preflight must reject a missing stable toolchain",
  );
  assert.ok(
    invokeCargo > requireStable,
    "missing stable must fail before Cargo runs",
  );
  assert.match(native, /Join-Path \$env:USERPROFILE '\.cargo\\bin'/);
  assert.match(native, /Join-Path \$cargoBin 'rustup\.exe'/);
  assert.match(native, /Join-Path \$cargoBin 'cargo\.exe'/);
  assert.match(native, /\$cargoBin \| Add-Content \$env:GITHUB_PATH/);
  assert.match(native, /\$toolchains = @\(& \$rustup toolchain list\)/);
  assert.match(
    native,
    /\$stableToolchain = \$toolchains \| Where-Object \{ \$_ -match '\^stable\(\?:-\|\\s\|\$\)' \} \| Select-Object -First 1/,
  );
  assert.match(native, /& \$cargo '\+stable' '--version'/);
});

test("native runner contains no Rust installer action", () => {
  const native = jobSource("native-whisper");

  assert.doesNotMatch(native, /dtolnay\/rust-toolchain/);
  assert.doesNotMatch(native, /rustup(?:-init)?(?:\.exe)?\s+(?:install|default|self update)/i);
});

test("hosted Miri job keeps its pinned nightly setup", () => {
  const miri = jobSource("pure-stt-state");

  assert.match(miri, /uses: dtolnay\/rust-toolchain@nightly/);
  assert.match(miri, /toolchain: nightly-2026-09-05/);
  assert.match(miri, /components: miri/);
  assert.match(miri, /cargo \+nightly-2026-09-05 miri setup/);
});

test("native preflight reports clear provisioning failures", () => {
  const native = jobSource("native-whisper");

  assert.match(native, /self-hosted runner Rust is not provisioned: missing rustup\.exe/);
  assert.match(native, /self-hosted runner Rust is not provisioned: missing cargo\.exe/);
  assert.match(native, /self-hosted runner Rust is not provisioned: rustup toolchain list failed/);
  assert.match(native, /self-hosted runner Rust is not provisioned: stable toolchain is missing/);
  assert.match(native, /self-hosted runner Rust is not provisioned: stable toolchain is unavailable/);
});
