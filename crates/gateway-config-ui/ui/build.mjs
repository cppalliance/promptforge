// Bundles src/main.ts into dist/app.js and copies the static assets into
// dist/. The crate's build.rs performs the same steps into OUT_DIR on
// `cargo build` (through the build-ui helper); this script exists for the
// fast iteration workflow (`npm run watch` rebuilds on save without a Rust
// recompile) and for the jsdom tests that import the built dist/app.js.
import { copyFile, mkdir, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "dist");
const srcDir = path.join(uiDir, "src");

// The crate version (workspace [workspace.package] version), baked into the
// bundle as __APP_VERSION__ for the Settings > About panel. A missing or
// unparsable workspace manifest falls back to the source's "dev" default.
async function crateVersion() {
  try {
    const manifest = await readFile(path.join(uiDir, "..", "..", "..", "Cargo.toml"), "utf8");
    return /^version\s*=\s*"([^"]+)"/m.exec(manifest)?.[1] ?? null;
  } catch {
    return null;
  }
}

const version = await crateVersion();

// Mirrored in the build-ui crate's CONFIG_UI_STATIC_FILES.
const STATIC_FILES = ["index.html", "icons/promptforge-icon-1.png"];

// Always minified: the bundle is never inspected by hand, and matching the
// release profile keeps the jsdom tests exercising what ships.
const options = {
  entryPoints: [path.join(srcDir, "main.ts")],
  bundle: true,
  format: "esm",
  target: "es2022",
  minify: true,
  outfile: path.join(distDir, "app.js"),
  logLevel: "info",
  ...(version !== null && { define: { __APP_VERSION__: JSON.stringify(version) } }),
};

// dist/ is rebuilt from scratch so removed assets never linger.
async function copyStatic() {
  await mkdir(distDir, { recursive: true });
  await Promise.all(
    STATIC_FILES.map(async (file) => {
      const target = path.join(distDir, file);
      await mkdir(path.dirname(target), { recursive: true });
      await copyFile(path.join(uiDir, file), target);
    }),
  );
}

if (process.argv.includes("--watch")) {
  const context = await esbuild.context(options);
  await copyStatic();
  await context.watch();
  console.log("watching ui/src for changes...");
} else {
  await rm(distDir, { recursive: true, force: true });
  await esbuild.build(options);
  await copyStatic();
}
