// Bundles src/main.ts into dist/app.js and copies the static assets into
// dist/. The server crate's build.rs performs the same two steps on
// `cargo build` (STATIC_FILES is mirrored there); this script exists for the
// fast iteration workflow: `npm run watch` rebuilds on save without a Rust
// recompile.
import { copyFile, mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "dist");

// Mirrored in ../build.rs.
const STATIC_FILES = ["index.html", "style.css", "pcm-worklet.js", "markdown-it.min.js"];

const options = {
  entryPoints: [path.join(uiDir, "src", "main.ts")],
  bundle: true,
  format: "esm",
  target: "es2022",
  outfile: path.join(distDir, "app.js"),
  logLevel: "info",
};

async function copyStatic() {
  await mkdir(distDir, { recursive: true });
  await Promise.all(
    STATIC_FILES.map((file) => copyFile(path.join(uiDir, file), path.join(distDir, file))),
  );
}

if (process.argv.includes("--watch")) {
  const context = await esbuild.context(options);
  await copyStatic();
  await context.watch();
  console.log("watching ui/src for changes...");
} else {
  await esbuild.build(options);
  await copyStatic();
}
