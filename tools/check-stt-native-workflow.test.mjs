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
const cargoManifest = readFileSync(join(root, "Cargo.toml"), "utf8");
const rustToolchain = readFileSync(join(root, "rust-toolchain.toml"), "utf8");

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

test("native runner validates the exact repository MSRV before caching", () => {
  const native = jobSource("native-whisper");
  const preflight = native.indexOf("- name: Verify preinstalled MSRV Rust");
  const cache = native.indexOf("- name: Cache Cargo");
  const resolveCargo = native.indexOf(
    "$cargo = Resolve-RustTool -Name 'cargo' -Bin $contractBin",
  );
  const resolveRustc = native.indexOf(
    "$rustc = Resolve-RustTool -Name 'rustc' -Bin $contractBin",
  );
  const validateCargo = native.indexOf(
    "Assert-RustToolVersion -Name 'cargo' -ToolPath $cargo",
  );
  const validateRustc = native.indexOf(
    "Assert-RustToolVersion -Name 'rustc' -ToolPath $rustc",
  );
  const publishContract = native.indexOf(
    "$contractBin | Add-Content -Path $env:GITHUB_PATH",
  );

  assert.ok(preflight > 0, "native job must have a Rust preflight");
  assert.ok(cache > preflight, "Rust preflight must run before Cargo caching");
  assert.ok(resolveCargo > preflight, "preflight must resolve cargo.exe");
  assert.ok(resolveRustc > resolveCargo, "preflight must resolve rustc.exe");
  assert.ok(validateCargo > resolveRustc, "preflight must validate Cargo");
  assert.ok(validateRustc > validateCargo, "preflight must validate rustc");
  assert.ok(
    publishContract > validateRustc,
    "contract bin must not reach PATH before both versions pass",
  );
  assert.ok(cache > publishContract, "both versions must pass before caching");
  assert.match(cargoManifest, /^rust-version = "1\.89"$/m);
  assert.match(rustToolchain, /^channel = "1\.89"$/m);
  assert.match(native, /^\s+RUSTUP_TOOLCHAIN: 1\.89\.0$/m);
  assert.match(native, /^\s+RUSTUP_AUTO_INSTALL: "0"$/m);
  assert.match(native, /\$requiredVersion = '1\.89\.0'/);
  assert.doesNotMatch(native, /RUSTUP_TOOLCHAIN: stable/);
});

test("direct tools come from PATH or the versioned runner contract", () => {
  const native = jobSource("native-whisper");

  assert.match(
    native,
    /\$contractName = 'PROMPTFORGE_RUST_1_89_0_BIN'/,
  );
  assert.match(native, /\[IO\.Path\]::IsPathRooted\(\$contractBin\)/);
  assert.match(native, /Test-Path \$contractBin -PathType Container/);
  assert.match(native, /\$candidate = Join-Path \$Bin "\$Name\.exe"/);
  assert.match(
    native,
    /Get-Command "\$Name\.exe" -CommandType Application -ErrorAction SilentlyContinue/,
  );
  assert.match(native, /return \$command\.Source/);
  assert.match(native, /\$contractBin \| Add-Content -Path \$env:GITHUB_PATH/);
  assert.doesNotMatch(native, /\$env:USERPROFILE/);
  assert.doesNotMatch(native, /\.cargo\\bin/);
});

test("rustup-managed PATH proxies use the exact preinstalled toolchain", () => {
  const native = jobSource("native-whisper");

  assert.match(native, /^\s+RUSTUP_TOOLCHAIN: 1\.89\.0$/m);
  assert.match(native, /^\s+RUSTUP_AUTO_INSTALL: "0"$/m);
  assert.match(native, /\$versionLines = @\(& \$ToolPath '--version' 2>&1\)/);
  assert.doesNotMatch(native, /missing rustup\.exe/);
  assert.doesNotMatch(native, /rustup toolchain list/);
  assert.doesNotMatch(native, /'\+stable'/);
});

test("native preflight rejects wrong and unrecognized tool versions", () => {
  const native = jobSource("native-whisper");

  assert.match(
    native,
    /\\s\+\(\\d\+\\\.\\d\+\\\.\\d\+\)\(\?:\\s\|\$\)/,
  );
  assert.match(native, /if \(-not \$versionMatch\.Success\)/);
  assert.match(native, /returned an unrecognized version at/);
  assert.match(native, /\$actualVersion = \$versionMatch\.Groups\[1\]\.Value/);
  assert.match(native, /if \(\$actualVersion -ne \$requiredVersion\)/);
  assert.match(
    native,
    /required repository MSRV is exactly \$requiredVersion/,
  );
  assert.match(native, /Reprovision it outside CI or point \$contractName/);
});

test("native preflight reports actionable missing-tool failures", () => {
  const native = jobSource("native-whisper");

  assert.match(
    native,
    /contract \$contractName is missing \$Name\.exe at '\$candidate'/,
  );
  assert.match(
    native,
    /\$Name\.exe was not found on PATH; install it outside CI or set \$contractName to its versioned bin directory/,
  );
  assert.match(native, /\$Name\.exe failed at '\$ToolPath' with exit code/);
  assert.match(native, /provision Rust \$requiredVersion outside CI/);
});

test("native runner contains no Rust installer action", () => {
  const native = jobSource("native-whisper");

  assert.doesNotMatch(native, /dtolnay\/rust-toolchain/);
  assert.doesNotMatch(
    native,
    /rustup(?:-init)?(?:\.exe)?\s+(?:install|default|self update)/i,
  );
});

test("hosted Miri job keeps its pinned nightly setup", () => {
  const miri = jobSource("pure-stt-state");

  assert.match(miri, /uses: dtolnay\/rust-toolchain@nightly/);
  assert.match(miri, /toolchain: nightly-2026-09-05/);
  assert.match(miri, /components: miri/);
  assert.match(miri, /cargo \+nightly-2026-09-05 miri setup/);
});

test("native fixture artifact hashes remain pinned", () => {
  const native = jobSource("native-whisper");

  assert.match(
    native,
    /F1BC54D7288E21EE826CCB5767249836B780FC316BEC4A0374873E73163DAE12/,
  );
  assert.match(
    native,
    /921E4CF8686FDD993DCD081A5DA5B6C365BFDE1162E72B08D75AC75289920B1F/,
  );
  assert.match(
    native,
    /59DFB9A4ACB36FE2A2AFFC14BACBEE2920FF435CB13CC314A08C13F66BA7860E/,
  );
});
