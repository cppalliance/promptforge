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
import { delimiter, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test, { after, before } from "node:test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(
  join(root, ".github", "workflows", "stt-miri.yml"),
  "utf8",
);
const cargoManifest = readFileSync(join(root, "Cargo.toml"), "utf8");
const rustToolchain = readFileSync(join(root, "rust-toolchain.toml"), "utf8");
const fixtureRoot = mkdtempSync(join(tmpdir(), "promptforge-rust-preflight-"));
const fakeToolSource = join(fixtureRoot, "fake-rust-tool.rs");
const fakeTool = join(
  fixtureRoot,
  process.platform === "win32" ? "fake-rust-tool.exe" : "fake-rust-tool",
);
const preflightScript = join(fixtureRoot, "preflight.ps1");

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

function stepScript(job, name) {
  const marker = `      - name: ${name}\n`;
  const start = job.indexOf(marker);
  assert.notEqual(start, -1, `missing ${name} step`);
  const remainder = job.slice(start + marker.length);
  const run = /^        run: \|\r?\n/m.exec(remainder);
  assert.ok(run, `missing run block for ${name}`);
  const body = remainder.slice(run.index + run[0].length);
  const nextStep = body.search(/^      - name: /m);
  const source = nextStep === -1 ? body : body.slice(0, nextStep);
  return source
    .split(/\r?\n/)
    .map((line) => line.startsWith("          ") ? line.slice(10) : line)
    .join("\n")
    .trimEnd();
}

function createToolLayout(names, { bin, proxy = false } = {}) {
  bin ??= mkdtempSync(
    join(fixtureRoot, proxy ? "proxy-bin-" : "direct-bin-"),
  );
  mkdirSync(bin, { recursive: true });
  for (const name of names) {
    const destination = join(bin, `${name}.exe`);
    copyFileSync(fakeTool, destination);
    chmodSync(destination, 0o755);
  }
  return bin;
}

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

function runPreflight({
  bin,
  discovery,
  explicitBin,
  windowsDirectory,
}) {
  const environment = { ...process.env };
  const runRoot = mkdtempSync(join(fixtureRoot, "preflight-run-"));
  const githubPath = join(runRoot, "github-path");
  const mode = discovery ?? (explicitBin ? "contract" : "path");

  deleteEnvironmentVariable(environment, "PROMPTFORGE_RUST_1_89_0_BIN");
  setEnvironmentVariable(environment, "GITHUB_PATH", githubPath);
  setEnvironmentVariable(environment, "RUSTUP_TOOLCHAIN", "1.89");
  setEnvironmentVariable(environment, "RUSTUP_AUTO_INSTALL", "0");

  if (mode === "contract") {
    setEnvironmentVariable(
      environment,
      "PROMPTFORGE_RUST_1_89_0_BIN",
      bin,
    );
  } else if (mode === "path") {
    const pathKey = environmentKey(environment, "PATH") ?? "PATH";
    setEnvironmentVariable(
      environment,
      "PATH",
      `${bin}${delimiter}${environment[pathKey] ?? ""}`,
    );
  } else if (mode === "network-service" || mode === "isolated") {
    const emptyPath = join(runRoot, "empty-path");
    const emptyProfile = join(runRoot, "empty-profile");
    mkdirSync(emptyPath, { recursive: true });
    mkdirSync(emptyProfile, { recursive: true });
    deleteEnvironmentVariable(environment, "CARGO_HOME");
    setEnvironmentVariable(environment, "PATH", emptyPath);
    setEnvironmentVariable(environment, "USERPROFILE", emptyProfile);
    setEnvironmentVariable(environment, "WINDIR", windowsDirectory);
  } else {
    throw new Error(`unknown preflight discovery mode: ${mode}`);
  }

  const executable = process.platform === "win32"
    ? join(
        process.env.SystemRoot ?? "C:\\WINDOWS",
        "System32",
        "WindowsPowerShell",
        "v1.0",
        "powershell.exe",
      )
    : "pwsh";
  const result = spawnSync(
    executable,
    ["-NoProfile", "-NonInteractive", "-File", preflightScript],
    {
      cwd: root,
      encoding: "utf8",
      env: environment,
      timeout: 30_000,
    },
  );
  return { ...result, githubPath };
}

before(() => {
  writeFileSync(
    fakeToolSource,
    String.raw`use std::env;

fn main() {
    let executable = env::current_exe().expect("current executable");
    let name = executable
        .file_stem()
        .expect("executable stem")
        .to_string_lossy()
        .to_ascii_lowercase();
    let is_proxy = executable
        .parent()
        .and_then(|parent| parent.file_name())
        .is_some_and(|parent| parent.to_string_lossy().starts_with("proxy-bin-"));
    if env::args().nth(1).is_some_and(|arg| arg.starts_with('+')) && !is_proxy {
        eprintln!("error: no such command: direct tool rejects rustup prefix");
        std::process::exit(2);
    }
    match name.as_str() {
        "cargo" => println!("cargo 1.89.0 (fixture 2026-09-07)"),
        "rustc" => println!("rustc 1.89.0 (fixture 2026-09-07)"),
        "rustup" => println!("rustup 1.28.2 (fixture 2026-09-07)"),
        _ => panic!("unexpected fake tool name: {name}"),
    }
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
  writeFileSync(
    preflightScript,
    `${stepScript(jobSource("native-whisper"), "Verify preinstalled MSRV Rust")}
if ($null -ne $LASTEXITCODE) { exit $LASTEXITCODE }
`,
  );
});

after(() => {
  rmSync(fixtureRoot, { force: true, recursive: true });
});

test("native runner validates the exact repository MSRV before caching", () => {
  const native = jobSource("native-whisper");
  const preflight = native.indexOf("- name: Verify preinstalled MSRV Rust");
  const cache = native.indexOf("- name: Cache Cargo");
  const resolveCargo = native.indexOf(
    "$cargo = Resolve-RustTool -Name 'cargo' -Bin $rustBin",
  );
  const resolveRustc = native.indexOf(
    "$rustc = Resolve-RustTool -Name 'rustc' -Bin $rustBin",
  );
  const validateCargo = native.indexOf(
    "Assert-RustToolVersion -Name 'cargo' -ToolPath $cargo",
  );
  const validateRustc = native.indexOf(
    "Assert-RustToolVersion -Name 'rustc' -ToolPath $rustc",
  );
  const publishContract = native.indexOf(
    "$rustBin | Add-Content -Path $env:GITHUB_PATH",
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
  assert.match(native, /^\s+RUSTUP_TOOLCHAIN: 1\.89$/m);
  assert.match(native, /^\s+RUSTUP_AUTO_INSTALL: "0"$/m);
  assert.match(native, /\$requiredVersion = '1\.89\.0'/);
  assert.doesNotMatch(native, /RUSTUP_TOOLCHAIN: stable/);
});

test("direct tools run from the versioned contract without rustup", () => {
  const bin = createToolLayout(["cargo", "rustc"]);
  const result = runPreflight({ bin, explicitBin: true });

  assert.equal(
    result.status,
    0,
    `direct preflight failed:\n${result.stdout}${result.stderr}`,
  );
  assert.match(result.stdout, /Using cargo 1\.89\.0 from /);
  assert.match(result.stdout, /Using rustc 1\.89\.0 from /);
  assert.match(result.stdout, /Using direct Rust tools/);
  assert.doesNotMatch(result.stdout, /Using rustup proxies/);
});

test("rustup-managed PATH proxies use the exact preinstalled toolchain", () => {
  const bin = createToolLayout(["cargo", "rustc", "rustup"], { proxy: true });
  const result = runPreflight({ bin, explicitBin: false });

  assert.equal(
    result.status,
    0,
    `proxy preflight failed:\n${result.stdout}${result.stderr}`,
  );
  assert.match(result.stdout, /Using cargo 1\.89\.0 from /);
  assert.match(result.stdout, /Using rustc 1\.89\.0 from /);
  assert.match(result.stdout, /Using rustup proxies from /);
});

test("NetworkService profile discovery executes without a repository variable", () => {
  const windowsDirectory = mkdtempSync(
    join(fixtureRoot, "windows-directory-"),
  );
  const serviceBin = join(
    windowsDirectory,
    "ServiceProfiles",
    "NetworkService",
    ".rustup",
    "toolchains",
    "1.89.0-x86_64-pc-windows-msvc",
    "bin",
  );
  createToolLayout(["cargo", "rustc"], { bin: serviceBin });

  const result = runPreflight({
    discovery: "network-service",
    windowsDirectory,
  });

  assert.equal(
    result.status,
    0,
    `NetworkService discovery failed:\n${result.stdout}${result.stderr}`,
  );
  assert.match(
    result.stdout,
    /Discovered preprovisioned Rust bin from NetworkService rustup toolchain 1\.89\.0-x86_64-pc-windows-msvc:/,
  );
  assert.match(result.stdout, /Using cargo 1\.89\.0 from /);
  assert.match(result.stdout, /Using rustc 1\.89\.0 from /);
  assert.equal(readFileSync(result.githubPath, "utf8").trim(), serviceBin);
});

test("tool discovery uses only explicit bounded candidate directories", () => {
  const native = jobSource("native-whisper");

  assert.match(
    native,
    /\$contractName = 'PROMPTFORGE_RUST_1_89_0_BIN'/,
  );
  assert.match(native, /\[IO\.Path\]::IsPathRooted\(\$contractBin\)/);
  assert.match(native, /Test-Path \$contractBin -PathType Container/);
  assert.match(
    native,
    /\$networkServiceProfile = Join-Path \$windowsDirectory 'ServiceProfiles\\NetworkService'/,
  );
  assert.match(
    native,
    /Join-Path \$networkServiceProfile '\.rustup'/,
  );
  assert.match(
    native,
    /"\$requiredVersion-x86_64-pc-windows-msvc"/,
  );
  assert.match(
    native,
    /"\$env:RUSTUP_TOOLCHAIN-x86_64-pc-windows-msvc"/,
  );
  assert.match(native, /-Source 'NetworkService service profile'/);
  assert.match(native, /Join-Path \$cargoHome 'bin'/);
  assert.match(native, /Join-Path \$userProfile '\.cargo\\bin'/);
  assert.match(native, /\$candidate = Join-Path \$Bin "\$Name\.exe"/);
  assert.match(
    native,
    /Get-Command 'cargo\.exe' -CommandType Application -ErrorAction SilentlyContinue/,
  );
  assert.match(native, /\$rustBin \| Add-Content -Path \$env:GITHUB_PATH/);
  assert.doesNotMatch(native, /Get-ChildItem/);
  assert.doesNotMatch(native, /-Recurse/);
});

test("rustup proxies remain pinned and cannot auto-install", () => {
  const native = jobSource("native-whisper");
  const proxyProbe = native.indexOf(
    "$cargoIsRustupProxy = Test-RustupProxy -ToolPath $cargo",
  );
  const resolveRustup = native.indexOf(
    "$rustup = Resolve-RustTool -Name 'rustup' -Bin $rustBin",
  );

  assert.match(native, /^\s+RUSTUP_TOOLCHAIN: 1\.89$/m);
  assert.match(native, /^\s+RUSTUP_AUTO_INSTALL: "0"$/m);
  assert.ok(proxyProbe > 0, "selected cargo must be checked as a proxy");
  assert.ok(
    resolveRustup > proxyProbe,
    "rustup must be resolved only after the selected cargo is a proxy",
  );
  assert.match(native, /\$versionLines = @\(& \$ToolPath '--version' 2>&1\)/);
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
    /no bounded candidate directory contained cargo\.exe and rustc\.exe/,
  );
  assert.match(
    native,
    /Provision both tools together outside CI or set \$contractName to their absolute versioned bin directory/,
  );
  assert.match(
    native,
    /selected bin from \$rustBinSource is missing \$Name\.exe at '\$candidate'/,
  );
  assert.match(native, /\$Name\.exe failed at '\$ToolPath' with exit code/);
  assert.match(native, /provision Rust \$requiredVersion outside CI/);
});

test("missing bounded candidates report every checked source", () => {
  const windowsDirectory = mkdtempSync(
    join(fixtureRoot, "empty-windows-directory-"),
  );
  const result = runPreflight({
    discovery: "isolated",
    windowsDirectory,
  });
  const output = `${result.stdout}${result.stderr}`.replace(/\s+/g, " ");

  assert.notEqual(result.status, 0, "missing tools must fail the preflight");
  assert.match(output, /no bounded candidate directory contained cargo\.exe and rustc\.exe/);
  assert.match(output, /USERPROFILE '/);
  assert.match(output, /NetworkService service profile '/);
  assert.match(output, /PROMPTFORGE_RUST_1_89_0_BIN/);
  assert.match(output, /Provision both tools together outside CI/);
});

test("native runner contains no Rust installer action", () => {
  const native = jobSource("native-whisper");

  assert.doesNotMatch(native, /dtolnay\/rust-toolchain/);
  assert.doesNotMatch(
    native,
    /rustup(?:-init)?(?:\.exe)?\s+(?:install|default|self update)/i,
  );
});

test("all native Whisper work stays on the Windows CUDA runner", () => {
  const native = jobSource("native-whisper");

  assert.match(native, /^\s+runs-on: \[self-hosted, windows, cuda\]$/m);
  assert.match(native, /Test safe Whisper backend integration/);
  assert.match(native, /Test native prompt budgets/);
  assert.match(native, /Test native Whisper FFI/);
  assert.match(native, /Test native Gateway STT units/);
  assert.match(native, /Test native Gateway STT integration/);
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
