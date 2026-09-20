// Bundles src/main.ts into dist/bundle/app-<hash>.js and copies the static
// assets into dist/. The crate's build.rs performs the same steps into
// OUT_DIR on `cargo build` (through the build-ui helper); this script
// exists for the fast iteration workflow (`npm run watch` rebuilds on save
// without a Rust recompile) and for the jsdom tests that import the built
// bundle. `--out <dir>` redirects the output (default dist/); the build-ui
// crate's drift test uses it to diff this script against the Rust
// implementer without touching the working tree.
import { copyFile, mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const outFlag = process.argv.indexOf("--out");
if (outFlag !== -1 && process.argv[outFlag + 1] === undefined) {
  throw new Error("--out requires a directory argument");
}
const distDir =
  outFlag === -1 ? path.join(uiDir, "dist") : path.resolve(uiDir, process.argv[outFlag + 1]);
const srcDir = path.join(uiDir, "src");

// The crate version (workspace [workspace.package] version), baked into the
// bundle as __APP_VERSION__ for the About dialog. A missing or unparsable
// workspace manifest falls back to the source's "dev" default.
async function crateVersion() {
  try {
    const manifest = await readFile(path.join(uiDir, "..", "..", "..", "Cargo.toml"), "utf8");
    return /^version\s*=\s*"([^"]+)"/m.exec(manifest)?.[1] ?? null;
  } catch {
    return null;
  }
}

const version = await crateVersion();

// Mirrored in the build-ui crate's WORKSHOP_STATIC_FILES.
const STATIC_FILES = [
  "index.html",
  "style.css",
  "pcm-worklet.js",
  "icons/promptforge-icon.png",
  "icons/promptforge-icon@2x.png",
];

// Always minified: the bundle is never inspected by hand, and matching the
// release profile keeps the jsdom tests exercising what ships.
//
// Code splitting is on: the panel registry's import thunks (the agent
// session's Shiki/TipTap graph, the editor's CodeMirror) become lazily
// loaded chunks under dist/chunks/, and the initial bundle carries only
// the boot shell, services, and chrome. Every bundle file is
// content-hashed (the entry under bundle/, the chunks under chunks/), so
// the server can mark them Cache-Control: immutable; dist/manifest.json
// maps the logical names (app.js, app.css) to the hashed files, and the
// dist copy of index.html is stamped with the hashed URLs.
const options = {
  entryPoints: [path.join(srcDir, "main.ts")],
  bundle: true,
  format: "esm",
  splitting: true,
  target: "es2022",
  minify: true,
  entryNames: "bundle/app-[hash]",
  chunkNames: "chunks/[name]-[hash]",
  outdir: distDir,
  logLevel: "info",
  plugins: [
    {
      name: "stamp-hashed-assets",
      setup(build) {
        build.onEnd(async (result) => {
          if (result.errors.length === 0) {
            await stampHashedAssets();
          }
        });
      },
    },
  ],
  ...(version !== null && { define: { __APP_VERSION__: JSON.stringify(version) } }),
};

// dist/ is rebuilt from scratch so removed assets never linger. The index
// page is excluded here: the stamp step writes it from the source with the
// hashed bundle URLs on every build, watch rebuilds included.
async function copyStatic() {
  await mkdir(distDir, { recursive: true });
  await Promise.all(
    STATIC_FILES.filter((file) => file !== "index.html").map(async (file) => {
      const target = path.join(distDir, file);
      await mkdir(path.dirname(target), { recursive: true });
      await copyFile(path.join(uiDir, file), target);
    }),
  );
}

// Finds the single hashed entry output of one kind under dist/bundle/.
async function hashedEntry(extension) {
  const bundleDir = path.join(distDir, "bundle");
  const names = (await readdir(bundleDir)).filter(
    (name) => name.startsWith("app-") && name.endsWith(extension),
  );
  if (names.length !== 1) {
    throw new Error(`expected exactly one bundle/app-*${extension} output, found ${names.length}`);
  }
  return `bundle/${names[0]}`;
}

// Writes dist/manifest.json (the logical-to-hashed name map the server's
// asset routes resolve through) and stamps the dist copy of index.html
// with the hashed bundle URLs, so the page loads the immutable assets
// directly. The stamp always reads the source index.html, so watch
// rebuilds never double-stamp. Mirrored in the build-ui crate's finalize
// step.
async function stampHashedAssets() {
  const manifest = {
    "app.js": await hashedEntry(".js"),
    "app.css": await hashedEntry(".css"),
  };
  await writeFile(path.join(distDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  const html = await readFile(path.join(uiDir, "index.html"), "utf8");
  const stamped = html
    .replace('href="/app.css"', `href="/${manifest["app.css"]}"`)
    .replace('src="/app.js"', `src="/${manifest["app.js"]}"`);
  if (stamped === html) {
    throw new Error("index.html did not reference /app.js and /app.css; the stamp found nothing");
  }
  await writeFile(path.join(distDir, "index.html"), stamped);
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
