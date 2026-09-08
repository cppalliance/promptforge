import {
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  rmSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function targetNames(target) {
  if (!/^[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,}$/.test(target)) {
    throw new Error(`invalid target triple: ${target}`);
  }
  const extension = target.split("-").includes("windows") ? ".exe" : "";
  return {
    binary: `promptforge-gateway${extension}`,
    sidecar: `promptforge-gateway-${target}${extension}`,
  };
}

export function gatewayBinaryName(target) {
  return targetNames(target).binary;
}

export function gatewaySidecarName(target) {
  return targetNames(target).sidecar;
}

function sidecarPath(root, target) {
  return join(
    root,
    "crates",
    "workshop",
    "binaries",
    gatewaySidecarName(target),
  );
}

export function stageGatewaySidecar({ root = REPOSITORY_ROOT, target, source }) {
  const expectedSourceName = gatewayBinaryName(target);
  const sourcePath = resolve(source);
  if (!existsSync(sourcePath)) {
    throw new Error(`Gateway source binary does not exist: ${sourcePath}`);
  }
  if (!lstatSync(sourcePath).isFile()) {
    throw new Error(`Gateway source binary is not a file: ${sourcePath}`);
  }
  if (basename(sourcePath) !== expectedSourceName) {
    throw new Error(
      `Gateway source binary must be named ${expectedSourceName} for ${target}`,
    );
  }

  const destination = sidecarPath(root, target);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(sourcePath, destination);
  return destination;
}

export function removeGatewaySidecar({ root = REPOSITORY_ROOT, target }) {
  const destination = sidecarPath(root, target);
  rmSync(destination, { force: true });
  return destination;
}

function parseArguments(args) {
  const [action, ...options] = args;
  if (action !== "stage" && action !== "remove") {
    throw new Error("usage: stage-gateway-sidecar.mjs <stage|remove> --target <triple> [--source <path>]");
  }

  const values = new Map();
  for (let index = 0; index < options.length; index += 2) {
    const name = options[index];
    const value = options[index + 1];
    if ((name !== "--target" && name !== "--source") || value === undefined) {
      throw new Error(`invalid sidecar argument: ${name ?? "<missing>"}`);
    }
    if (values.has(name)) {
      throw new Error(`duplicate sidecar argument: ${name}`);
    }
    values.set(name, value);
  }

  const target = values.get("--target");
  if (target === undefined) {
    throw new Error("missing required sidecar argument: --target");
  }
  const source = values.get("--source");
  if (action === "stage" && source === undefined) {
    throw new Error("missing required sidecar argument: --source");
  }
  if (action === "remove" && source !== undefined) {
    throw new Error("remove does not accept --source");
  }
  return { action, source, target };
}

function main(args) {
  const { action, source, target } = parseArguments(args);
  const path =
    action === "stage"
      ? stageGatewaySidecar({ source, target })
      : removeGatewaySidecar({ target });
  console.log(`${action === "stage" ? "staged" : "removed"} ${path}`);
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
