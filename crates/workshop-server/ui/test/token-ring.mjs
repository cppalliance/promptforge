// The token ring (src/ui/token-ring.ts) in jsdom: an SVG gauge with a
// track circle and a progress circle whose stroke-dashoffset encodes
// the context-usage percentage. The default provider stub returns 0%
// (an empty ring); an injected provider or setPercentage drives
// non-zero values, clamped to 0-100. Runs under the shared leak check:
// a TokenRing left undisposed fails.
// Run: node test/token-ring.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";
import { assertNoLeaks } from "./helpers/leak-check.mjs";

const testDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export * as lifecycle from "./src/base/lifecycle.ts";
      export { TokenRing } from "./src/ui/token-ring.ts";
    `,
    resolveDir: path.join(testDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module under test imports its colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7910/",
});
globalThis.window = dom.window;
globalThis.document = dom.window.document;
globalThis.Element = dom.window.Element;
globalThis.Node = dom.window.Node;

const bundlePath = path.join(os.tmpdir(), "promptforge-token-ring-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { TokenRing, lifecycle } = await import(pathToFileURL(bundlePath).href);

// The geometry the component fixes: 16px viewBox, stroke width 2.
const RADIUS = 7;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

function closeTo(actual, expected) {
  return Math.abs(actual - expected) < 1e-9;
}

function circles(ring) {
  return [...ring.element.querySelectorAll("circle")];
}

await assertNoLeaks(lifecycle, async () => {
  // --- Structure -------------------------------------------------------------

  {
    const ring = new TokenRing();
    document.body.appendChild(ring.element);
    check(
      "the ring is an svg carrying the token-ring class",
      ring.element.tagName === "svg" &&
        ring.element.getAttribute("class") === "token-ring",
    );
    check(
      "the svg uses the 16px viewBox",
      ring.element.getAttribute("viewBox") === "0 0 16 16",
    );
    const [background, progress] = circles(ring);
    check(
      "the ring renders a background circle then a progress circle",
      background?.getAttribute("class") === "token-ring-background" &&
        progress?.getAttribute("class") === "token-ring-progress",
    );
    check(
      "both circles share the center, radius, and stroke width",
      circles(ring).every(
        (circle) =>
          circle.getAttribute("cx") === "8" &&
          circle.getAttribute("cy") === "8" &&
          circle.getAttribute("r") === String(RADIUS) &&
          circle.getAttribute("stroke-width") === "2",
      ),
    );
    check(
      "only the progress circle carries the dash wiring",
      background?.getAttribute("stroke-dasharray") === null &&
        closeTo(Number(progress?.getAttribute("stroke-dasharray")), CIRCUMFERENCE),
    );
    ring.dispose();
    ring.element.remove();
  }

  // --- Accessibility -----------------------------------------------------------

  {
    const ring = new TokenRing();
    check(
      "the ring is a labeled progressbar over 0-100",
      ring.element.getAttribute("role") === "progressbar" &&
        ring.element.getAttribute("aria-label") === "Context usage" &&
        ring.element.getAttribute("aria-valuemin") === "0" &&
        ring.element.getAttribute("aria-valuemax") === "100",
    );
    ring.dispose();
  }

  // --- The stub ------------------------------------------------------------------

  {
    const ring = new TokenRing();
    check("the default provider reads as 0%", ring.percentage === 0);
    check(
      "the stub percentage shows an empty ring",
      closeTo(
        Number(
          ring.element
            .querySelector(".token-ring-progress")
            ?.getAttribute("stroke-dashoffset"),
        ),
        CIRCUMFERENCE,
      ) && ring.element.getAttribute("aria-valuenow") === "0",
    );
    ring.dispose();
  }

  // --- Non-zero percentages --------------------------------------------------------

  {
    const ring = new TokenRing(() => 25);
    const progress = ring.element.querySelector(".token-ring-progress");
    check(
      "an injected provider sets the initial percentage",
      ring.percentage === 25 && ring.element.getAttribute("aria-valuenow") === "25",
    );
    check(
      "25% fills a quarter of the circumference",
      closeTo(Number(progress?.getAttribute("stroke-dashoffset")), CIRCUMFERENCE * 0.75),
    );

    ring.setPercentage(50);
    check(
      "setPercentage re-renders the arc and the aria value",
      ring.percentage === 50 &&
        ring.element.getAttribute("aria-valuenow") === "50" &&
        closeTo(Number(progress?.getAttribute("stroke-dashoffset")), CIRCUMFERENCE * 0.5),
    );

    ring.setPercentage(140);
    check(
      "percentages above 100 clamp to a full ring",
      ring.percentage === 100 &&
        closeTo(Number(progress?.getAttribute("stroke-dashoffset")), 0),
    );

    ring.setPercentage(-10);
    check(
      "percentages below 0 clamp to an empty ring",
      ring.percentage === 0 &&
        closeTo(Number(progress?.getAttribute("stroke-dashoffset")), CIRCUMFERENCE),
    );

    ring.setPercentage(Number.NaN);
    check(
      "a non-finite percentage reads as an empty ring",
      ring.percentage === 0 &&
        ring.element.getAttribute("aria-valuenow") === "0" &&
        closeTo(Number(progress?.getAttribute("stroke-dashoffset")), CIRCUMFERENCE),
    );
    ring.dispose();

    const nanRing = new TokenRing(() => Number.NaN);
    check(
      "a non-finite provider value reads as 0%",
      nanRing.percentage === 0 &&
        nanRing.element.getAttribute("aria-valuenow") === "0",
    );
    nanRing.dispose();
  }
});

if (failures.length > 0) {
  console.error(`token-ring: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("token-ring: all assertions passed");
process.exit(0);
