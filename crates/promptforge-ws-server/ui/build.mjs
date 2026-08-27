// Bundles src/main.ts into dist/app.js and copies the static assets into
// dist/. The server crate's build.rs performs the same two steps on
// `cargo build` (STATIC_FILES is mirrored there); this script exists for the
// fast iteration workflow: `npm run watch` rebuilds on save without a Rust
// recompile.
import { copyFile, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { checkImport } from "./check-layers.mjs";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.join(uiDir, "dist");
const srcDir = path.join(uiDir, "src");
const chatDir = path.join(srcDir, "chat");

// Mirrored in ../build.rs.
const STATIC_FILES = ["index.html", "style.css", "pcm-worklet.js", "icons/promptforge-icon-1.png"];

// The layer rule (defined once, in check-layers.mjs) enforced while
// bundling, so `esbuild.build` and watch mode fail on a violating import.
// Only relative imports from workshop-owned files under src/ are checked;
// package imports and the vendored chat/ tree are exempt.
const layerCheckPlugin = {
  name: "check-layers",
  setup(build) {
    build.onResolve({ filter: /.*/ }, (args) => {
      if (!args.importer || (args.namespace !== "file" && args.namespace !== "")) {
        return null;
      }
      if (!args.path.startsWith(".")) {
        return null;
      }
      const importer = path.resolve(args.importer);
      if (!importer.startsWith(srcDir + path.sep) || importer.startsWith(chatDir + path.sep)) {
        return null;
      }
      const violation = checkImport(importer, path.resolve(args.resolveDir, args.path));
      // Returning null hands the allowed import back to esbuild's own
      // resolver; the plugin only ever vetoes.
      return violation === null ? null : { errors: [{ text: violation }] };
    });
  },
};

const options = {
  entryPoints: [path.join(srcDir, "main.ts")],
  bundle: true,
  format: "esm",
  target: "es2022",
  // `node build.mjs --minify` matches what build.rs does for cargo release
  // builds.
  minify: process.argv.includes("--minify"),
  outfile: path.join(distDir, "app.js"),
  logLevel: "info",
  plugins: [layerCheckPlugin],
};

// dist/ is rebuilt from scratch so removed assets never linger into the
// release embed.
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
