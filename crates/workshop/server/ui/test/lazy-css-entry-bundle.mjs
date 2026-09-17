// Lazy-CSS entry-bundle contract: the lazy feature directories (agent,
// editor, gateway, stt) import their stylesheets beside their TypeScript,
// and esbuild hoists CSS reachable through dynamic imports into the
// entry's app-*.css - no eager re-import in main.ts is needed or wanted.
// This test builds for real and asserts over the build output: the entry
// stylesheet must carry one marker class per lazy directory, so a future
// esbuild behavior change (or an accidental import-graph cut) that drops
// a lazy directory's CSS fails here instead of shipping unstyled panels.
// The build goes to a scratch directory through build.mjs --out: the
// node --test runner executes the suite in parallel, and rebuilding dist/
// here would race the boot tests reading it.
// Run: node test/lazy-css-entry-bundle.mjs
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const outDir = await mkdtemp(path.join(os.tmpdir(), "workshop-lazy-css-"));

try {
  const build = spawnSync(process.execPath, ["build.mjs", "--out", outDir], {
    cwd: uiDir,
    encoding: "utf8",
  });
  if (build.status !== 0) {
    console.error(build.stdout);
    console.error(build.stderr);
    console.error(`lazy-css-entry-bundle: the build failed (status ${build.status})`);
    process.exit(1);
  }

  const manifest = JSON.parse(await readFile(path.join(outDir, "manifest.json"), "utf8"));
  const css = await readFile(path.join(outDir, manifest["app.css"]), "utf8");

  // One marker class per lazy feature directory: a class that only that
  // directory's colocated stylesheet defines.
  const markers = {
    agent: "ws-agent-session",
    editor: "ws-editor-panel",
    gateway: "ws-gateway-config-panel",
    stt: "ws-stt-mic",
  };

  const failures = [];
  for (const [directory, marker] of Object.entries(markers)) {
    if (!css.includes(`.${marker}`)) {
      failures.push(`the entry stylesheet is missing .${marker} (the ${directory} directory's CSS)`);
    }
  }

  if (failures.length > 0) {
    console.error(`lazy-css-entry-bundle: ${failures.length} failure(s)`);
    for (const failure of failures) console.error(`  - ${failure}`);
    process.exit(1);
  }
  console.log("lazy-css-entry-bundle: all assertions passed");
} finally {
  await rm(outDir, { recursive: true, force: true });
}
