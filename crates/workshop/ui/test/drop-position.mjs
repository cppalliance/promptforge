// The dock's drop-position resolver and theme (src/parts/layout/drop-position.ts
// and src/parts/layout/dock-theme.ts), the two options main.ts hands Dockview
// for Cursor's drag-to-dock behavior. The drop target is cut in thirds, and
// the middle ninth is the center. Where two edges claim the pointer (a
// corner), the nearer one wins, measured against the target's own size. A
// pointer within 10% of an edge is inside that edge's third and nearer it than
// any other edge, so it docks there with no rule of its own (a band branch
// ahead of the thirds could never change an answer). A target that does not
// accept the chosen position falls back to the center, and one that accepts
// nothing answers null. The theme is the dark theme with the thin insertion
// line for tab drops.
// Run: node test/drop-position.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";

const uiDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { dropPositionResolver, resolveDropPosition } from "./src/parts/layout/drop-position.ts";
      export { dockTheme } from "./src/parts/layout/dock-theme.ts";
      export { themeDark } from "dockview";
    `,
    resolveDir: uiDir,
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  loader: { ".css": "empty" },
});
const bundlePath = path.join(os.tmpdir(), "promptforge-drop-position-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { dropPositionResolver, resolveDropPosition, dockTheme, themeDark } = await import(
  pathToFileURL(bundlePath).href
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const ALL = new Set(["top", "bottom", "left", "right", "center"]);
/** The position a pointer at (x, y) percent of a 1000 x 600 target resolves to. */
function at(xPercent, yPercent, zones = ALL) {
  const result = resolveDropPosition({
    x: (xPercent / 100) * 1000,
    y: (yPercent / 100) * 600,
    width: 1000,
    height: 600,
    zones,
    event: {},
  });
  return result === null ? null : result.position;
}

// --- The middle ninth is the center -------------------------------------------------

check("the middle of the target is the center", at(50, 50) === "center");
check("just inside the middle ninth is still the center", at(40, 40) === "center" && at(60, 60) === "center");

// --- A pointer within a tenth of an edge docks against that edge -------------------
// No rule of its own: such a pointer is in that edge's third and nearer it than
// any other edge, so the thirds below already send it there.

check("the left edge docks left", at(3, 50) === "left");
check("the right edge docks right", at(97, 50) === "right");
check("the top edge docks top", at(50, 2) === "top");
check("the bottom edge docks bottom", at(50, 98) === "bottom");
{
  // Wherever along an edge the pointer is, up to a tenth in from it (clear of the
  // corners, where two edges are that close and the nearer one wins instead).
  let wrong = 0;
  for (let along = 15; along <= 85; along += 5) {
    for (const across of [0, 1, 5, 9.9]) {
      if (at(across, along) !== "left") wrong += 1;
      if (at(100 - across, along) !== "right") wrong += 1;
      if (at(along, across) !== "top") wrong += 1;
      if (at(along, 100 - across) !== "bottom") wrong += 1;
    }
  }
  check("a pointer within a tenth of an edge docks there all along that edge", wrong === 0);
}
check(
  "a pointer is measured against the target's own size, not in pixels",
  // 20% across a 1000px target is 200px from the left edge and 25% down a 600px target is
  // 150px from the top edge: pixels would pick the top, fractions pick the nearer edge, the left.
  at(20, 25) === "left",
);
// --- Outside the bands the target is cut in thirds ----------------------------------

check("the left third docks left", at(20, 50) === "left");
check("the right third docks right", at(80, 50) === "right");
check("the top third docks top", at(50, 20) === "top");
check("the bottom third docks bottom", at(50, 80) === "bottom");
check("a corner cell goes to the nearer edge: nearer the top", at(30, 15) === "top");
check("a corner cell goes to the nearer edge: nearer the left", at(15, 30) === "left");
check("a corner cell goes to the nearer edge: nearer the right", at(85, 30) === "right");
check("a corner cell goes to the nearer edge: nearer the bottom", at(70, 85) === "bottom");

// --- A target accepts only some positions --------------------------------------------

const sidesOnly = new Set(["left", "right", "center"]);
check("a position the target accepts is kept", at(10, 50, sidesOnly) === "left");
check("a position the target refuses falls back to the center", at(50, 5, sidesOnly) === "center");
check("a center-only target takes every pointer as the center", at(2, 2, new Set(["center"])) === "center");
check("a target accepting nothing answers null", at(50, 50, new Set()) === null);
check(
  "a target with no area is the center",
  resolveDropPosition({ x: 0, y: 0, width: 0, height: 0, zones: ALL, event: {} })?.position === "center",
);

// --- The resolver object is what Dockview takes --------------------------------------

check(
  "the exported resolver resolves through the same function",
  dropPositionResolver.resolve({ x: 30, y: 300, width: 1000, height: 600, zones: ALL, event: {} })?.position ===
    "left",
);
check("a resolved position carries no edge marker", at(3, 50) === "left" && resolveDropPosition({ x: 30, y: 300, width: 1000, height: 600, zones: ALL, event: {} })?.edge === undefined);

// --- The theme: dark, with the thin insertion line -----------------------------------

check("the dock theme shows tab drops as a thin line", dockTheme.dndTabIndicator === "line");
check(
  "the dock theme is the dark theme otherwise",
  dockTheme.className === themeDark.className && dockTheme.colorScheme === themeDark.colorScheme,
);

if (failures.length > 0) {
  console.error(`drop-position: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("drop-position: all assertions passed");
process.exit(0);
