// Writes the versioned artifact manifest (dist/manifest.json) for a
// packaged UI build. The server crate's build.rs verifies the manifest
// before embedding dist/ into a release binary, so the input-hash
// algorithm here is mirrored exactly in ../build/manifest.rs: sha256 over
// the byte-sorted, ui-relative forward-slash paths of every build input,
// feeding path bytes, a 0x00, the content bytes, and a 0x00 per file.
import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

// Manifest schema version; bump when the fields change. Mirrored in
// ../build/manifest.rs.
export const MANIFEST_VERSION = 1;

// Build scripts and manifests whose contents change the bundle without
// touching src/. Mirrored in ../build/manifest.rs.
const BUILD_INPUTS = [
  "build.mjs",
  "manifest.mjs",
  "check-layers.mjs",
  "package.json",
  "package-lock.json",
  "tsconfig.json",
];

// Collects every file under dir, as uiDir-relative forward-slash paths.
async function listTree(dir, uiDir, out) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      await listTree(full, uiDir, out);
    } else {
      out.push(path.relative(uiDir, full).split(path.sep).join("/"));
    }
  }
}

// Byte-wise sort; the paths are ASCII, so code-unit order matches the
// Rust side's byte order.
function byBytes(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

// Hashes every input the bundle depends on: src/**, the static files, and
// the build scripts and manifests. Any change to any of them must
// invalidate a packaged artifact.
export async function computeInputHash(uiDir, staticFiles) {
  const inputs = [];
  await listTree(path.join(uiDir, "src"), uiDir, inputs);
  inputs.push(...staticFiles, ...BUILD_INPUTS);
  inputs.sort(byBytes);
  const hash = createHash("sha256");
  for (const rel of inputs) {
    hash.update(rel, "utf8");
    hash.update(Buffer.from([0]));
    hash.update(await readFile(path.join(uiDir, rel)));
    hash.update(Buffer.from([0]));
  }
  return hash.digest("hex");
}

// Writes dist/manifest.json for the dist/ tree as it stands: the schema
// version, the minified flag, the input hash, and the sorted dist file
// list (excluding the manifest itself).
export async function writeManifest(uiDir, distDir, staticFiles) {
  const files = [];
  await listTree(distDir, distDir, files);
  const manifest = {
    version: MANIFEST_VERSION,
    minified: true,
    inputHash: await computeInputHash(uiDir, staticFiles),
    files: files.filter((file) => file !== "manifest.json").sort(byBytes),
  };
  await writeFile(path.join(distDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
}
