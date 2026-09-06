import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  gatewayBinaryName,
  gatewaySidecarName,
  removeGatewaySidecar,
  stageGatewaySidecar,
} from "./stage-gateway-sidecar.mjs";

test("maps the Windows target to Tauri's suffixed executable name", () => {
  assert.equal(
    gatewaySidecarName("x86_64-pc-windows-msvc"),
    "promptforge-gateway-x86_64-pc-windows-msvc.exe",
  );
  assert.equal(
    gatewayBinaryName("x86_64-pc-windows-msvc"),
    "promptforge-gateway.exe",
  );
});

test("maps the Linux target to Tauri's suffix without an extension", () => {
  assert.equal(
    gatewaySidecarName("x86_64-unknown-linux-gnu"),
    "promptforge-gateway-x86_64-unknown-linux-gnu",
  );
  assert.equal(
    gatewayBinaryName("x86_64-unknown-linux-gnu"),
    "promptforge-gateway",
  );
});

test("rejects an unsupported target", () => {
  assert.throws(
    () => gatewaySidecarName("aarch64-apple-darwin"),
    /unsupported Gateway sidecar target/,
  );
});

test("rejects a missing source binary", () => {
  const root = mkdtempSync(join(tmpdir(), "promptforge-sidecar-"));
  try {
    assert.throws(
      () =>
        stageGatewaySidecar({
          root,
          target: "x86_64-pc-windows-msvc",
          source: join(root, "target", "debug", "promptforge-gateway.exe"),
        }),
      /source binary does not exist/,
    );
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test("rejects a source binary whose platform name mismatches the target", () => {
  const root = mkdtempSync(join(tmpdir(), "promptforge-sidecar-"));
  const source = join(root, "target", "debug", "promptforge-gateway");
  try {
    mkdirSync(join(root, "target", "debug"), { recursive: true });
    writeFileSync(source, "linux gateway");
    assert.throws(
      () =>
        stageGatewaySidecar({
          root,
          target: "x86_64-pc-windows-msvc",
          source,
        }),
      /source binary must be named promptforge-gateway\.exe/,
    );
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});

test("stages and removes the real source file under Tauri's target name", () => {
  const root = mkdtempSync(join(tmpdir(), "promptforge-sidecar-"));
  const source = join(root, "target", "debug", "promptforge-gateway.exe");
  try {
    mkdirSync(join(root, "target", "debug"), { recursive: true });
    writeFileSync(source, "compiled gateway");

    const staged = stageGatewaySidecar({
      root,
      target: "x86_64-pc-windows-msvc",
      source,
    });
    assert.equal(
      staged,
      join(
        root,
        "crates",
        "workshop",
        "binaries",
        "promptforge-gateway-x86_64-pc-windows-msvc.exe",
      ),
    );
    assert.equal(readFileSync(staged, "utf8"), "compiled gateway");

    assert.equal(
      removeGatewaySidecar({
        root,
        target: "x86_64-pc-windows-msvc",
      }),
      staged,
    );
    assert.throws(() => readFileSync(staged), /ENOENT/);
    assert.equal(
      removeGatewaySidecar({
        root,
        target: "x86_64-pc-windows-msvc",
      }),
      staged,
    );
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});
