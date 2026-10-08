// The token ring (src/parts/chrome/token-ring.ts) in jsdom: a 15px SVG gauge
// (stroke 2, radius 5.5, round line caps) with a track circle and a progress
// circle whose stroke-dashoffset encodes the context-usage percentage. The
// default provider stub reports no usage data (null), which hides the ring
// and drops its value; an injected provider or setPercentage drives the
// percentages, clamped to 0-100, and null hides the ring again. The
// stylesheet's side of the contract - the 15px size in a 20px box, the 8%
// track, hiding below 260px of composer - is read from the source text.
// Runs under the shared leak check: a TokenRing left undisposed fails.
// Run: node test/token-ring.mjs
import { readFile, writeFile } from "node:fs/promises";
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
      export * as lifecycle from "@workshop/platform/lifecycle";
      export { TokenRing } from "./src/parts/chrome/token-ring.ts";
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

// The geometry the component fixes: a 15-unit viewBox, radius 5.5, stroke width 2.
const RADIUS = 5.5;
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
      "the ring is an svg with the ws-token-ring class",
      ring.element.tagName === "svg" &&
        ring.element.getAttribute("class") === "ws-token-ring",
    );
    check(
      "the svg uses the 15px viewBox",
      ring.element.getAttribute("viewBox") === "0 0 15 15",
    );
    const [background, progress] = circles(ring);
    check(
      "the ring renders a background circle then a progress circle",
      background?.getAttribute("class") === "ws-token-ring-background" &&
        progress?.getAttribute("class") === "ws-token-ring-progress",
    );
    check(
      "both circles share the center, radius 5.5, and stroke width 2",
      circles(ring).every(
        (circle) =>
          circle.getAttribute("cx") === "7.5" &&
          circle.getAttribute("cy") === "7.5" &&
          circle.getAttribute("r") === String(RADIUS) &&
          circle.getAttribute("stroke-width") === "2",
      ),
    );
    check(
      "only the progress circle gets the dash wiring and round caps",
      background?.getAttribute("stroke-dasharray") === null &&
        closeTo(Number(progress?.getAttribute("stroke-dasharray")), CIRCUMFERENCE) &&
        progress?.getAttribute("stroke-linecap") === "round",
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

  // --- The stub: no usage data hides the ring ---------------------------------------

  {
    const ring = new TokenRing();
    check("the default provider reports no usage data", ring.percentage === null);
    check(
      "with no usage data the ring is hidden and reports no value",
      ring.element.hasAttribute("hidden") && !ring.element.hasAttribute("aria-valuenow"),
    );
    ring.setPercentage(40);
    check(
      "usage data shows the ring and its value",
      !ring.element.hasAttribute("hidden") &&
        ring.element.getAttribute("aria-valuenow") === "40",
    );
    ring.setPercentage(null);
    check(
      "null hides the ring again",
      ring.percentage === null &&
        ring.element.hasAttribute("hidden") &&
        !ring.element.hasAttribute("aria-valuenow"),
    );
    ring.dispose();
  }

  // --- Percentages ----------------------------------------------------------------------

  {
    const ring = new TokenRing(() => 25);
    const progress = ring.element.querySelector(".ws-token-ring-progress");
    check(
      "an injected provider sets the initial percentage and shows the ring",
      ring.percentage === 25 &&
        ring.element.getAttribute("aria-valuenow") === "25" &&
        !ring.element.hasAttribute("hidden"),
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

    const nullRing = new TokenRing(() => null);
    check("a null provider value is no data", nullRing.percentage === null && nullRing.element.hasAttribute("hidden"));
    nullRing.dispose();
  }
});

// --- The stylesheet --------------------------------------------------------------------

{
  const css = (await readFile(path.join(testDir, "..", "src", "parts", "chrome", "token-ring.css"), "utf8")).replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  );
  const lookTokens = await readFile(
    path.join(testDir, "..", "..", "look", "tokens.css"),
    "utf8",
  );
  check("the ring is 15px", /--token-ring-size:\s*15px/.test(lookTokens) && /inline-size:\s*var\(--token-ring-size\)/.test(css));
  check("the ring's track is the 8% stroke", /--token-ring-track:\s*var\(--cursor-stroke-tertiary\)/.test(lookTokens));
  check("the ring sits in a 20px box: the 2.5px margin around the 15px gauge", /margin:\s*var\(--ws-token-ring-margin\)/.test(css));
  check("the progress arc has round line caps", /stroke-linecap:\s*round/.test(css));
  check("a ring with no data stays hidden", /\.ws-token-ring\[hidden\]\s*\{\s*display:\s*none/.test(css));
  check(
    "the ring hides below 260px of composer",
    /@container ws-composer \(width < 260px\)\s*\{\s*\.ws-token-ring\s*\{\s*display:\s*none/.test(css),
  );
}

if (failures.length > 0) {
  console.error(`ws-token-ring: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("ws-token-ring: all assertions passed");
process.exit(0);
