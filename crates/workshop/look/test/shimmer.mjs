// Unit test for the shared shimmer (shimmer.ts and shimmer.css). The sweep's
// phase comes from one module start time, so every shimmering element is in
// phase and a rebuilt element does not restart the sweep: two elements
// started at different times, and a rebuilt copy, all read the same phase.
// The stylesheet carries the tunable variables, the three 1s transcript
// tones, and the reduced-motion contract (the shimmer stops and the row
// keeps its own resting color). Bundles the module with esbuild and drives
// it against jsdom with a scripted clock.
// Run: node test/shimmer.mjs (from crates/workshop/look).
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const lookDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Element = window.Element;
globalThis.Node = window.Node;

const bundle = await esbuild.build({
  entryPoints: [path.join(lookDir, "shimmer.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module may import its colocated CSS; the test drives only the JS.
  loader: { ".css": "empty" },
});

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The module reads its start time at load, so the clock is scripted before
// the import: the module starts at START, and every call below runs at a
// time the test chooses.
const START = 1_000_000;
let clock = START;
const realNow = Date.now;
Date.now = () => clock;
const { setShimmer } = await import(
  `data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`
);
const at = (offsetMs) => {
  clock = START + offsetMs;
};

const make = () => window.document.body.appendChild(window.document.createElement("span"));

// --- One shared phase -----------------------------------------------------------

at(300);
const firstStartedAt = clock;
const first = make();
setShimmer(first, true);
check("setShimmer(on) adds the shimmer class", first.classList.contains("ws-shimmer-text"));
check("the delay is negative and measured from the module start", first.style.animationDelay === "-300ms");

at(4_500);
const secondStartedAt = clock;
const second = make();
setShimmer(second, true);
check("a later element's delay is the elapsed time within the 2s loop", second.style.animationDelay === "-500ms");

// An animation started at time t with delay -d is d ms into the loop at t, so
// two elements are in phase when (t - d) agrees modulo the loop length. Both
// t and d are read back from what setShimmer actually set, not recomputed.
const period = 2000;
const delayOf = (element) => -Number.parseFloat(element.style.animationDelay);
const phaseOrigin = (startedAt, element) => startedAt - delayOf(element);
const modulo = (value) => ((value % period) + period) % period;
check(
  "two elements started at different times share one phase",
  modulo(phaseOrigin(firstStartedAt, first) - phaseOrigin(secondStartedAt, second)) === 0,
);

at(4_500);
const rebuilt = make();
setShimmer(rebuilt, true);
check("a rebuilt element gets the delay of an existing one started at the same time", rebuilt.style.animationDelay === second.style.animationDelay);

// --- A running element is not restarted -----------------------------------------

at(7_100);
setShimmer(second, true);
check("setShimmer(on) on a running element leaves its delay alone", second.style.animationDelay === "-500ms");

// --- Off ------------------------------------------------------------------------

setShimmer(second, false);
check("setShimmer(off) removes the shimmer class", !second.classList.contains("ws-shimmer-text"));
check("setShimmer(off) clears the delay", second.style.animationDelay === "");
at(7_300);
setShimmer(second, true);
check("turning it back on takes the shared phase again", second.style.animationDelay === `-${(7_300) % period}ms`);

// --- The element's own duration -------------------------------------------------

at(2_700);
const custom = make();
custom.style.setProperty("--shimmer-duration", "1s");
setShimmer(custom, true);
check("an element's --shimmer-duration sets the loop the delay wraps in", custom.style.animationDelay === "-700ms");

const millis = make();
millis.style.setProperty("--shimmer-duration", "500ms");
setShimmer(millis, true);
check("a duration in milliseconds is read too", millis.style.animationDelay === "-200ms");

// A tone modifier is a 1s loop in the stylesheet; a detached element has no
// computed style, so the module knows the tones itself.
for (const tone of ["status", "thinking", "tool"]) {
  const toned = window.document.createElement("span");
  toned.className = `ws-shimmer-text--${tone}`;
  setShimmer(toned, true);
  check(`the ${tone} tone wraps the delay in its 1s loop`, toned.style.animationDelay === "-700ms");
}

Date.now = realNow;

// --- shimmer.css ----------------------------------------------------------------

const css = await readFile(path.join(lookDir, "shimmer.css"), "utf8");
const code = css.replace(/\/\*[\s\S]*?\*\//g, "");
const [mainCss, reducedCss = ""] = code.split("@media (prefers-reduced-motion: reduce)");
const flat = (text) => text.replace(/\s+/g, " ");

function declarations(text, selector) {
  const out = {};
  const pattern = /([^{}]+)\{([^{}]*)\}/g;
  for (const match of text.matchAll(pattern)) {
    const selectors = match[1].split(",").map((entry) => flat(entry).trim());
    if (!selectors.includes(selector)) continue;
    for (const declaration of match[2].split(";")) {
      const colon = declaration.indexOf(":");
      if (colon < 0) continue;
      out[declaration.slice(0, colon).trim()] = flat(declaration.slice(colon + 1)).trim();
    }
  }
  return out;
}

const base = declarations(mainCss, ".ws-shimmer-text");
check(
  "the gradient reads --shimmer-base and --shimmer-peak",
  (base["background-image"] ?? "").includes("var(--shimmer-base,") &&
    (base["background-image"] ?? "").includes("var(--shimmer-peak,"),
);
check(
  "the base defaults to the primary text at 60%",
  (base["background-image"] ?? "").includes("var(--shimmer-base, color-mix(in srgb, var(--cursor-text-primary) 60%, transparent))"),
);
check(
  "the peak defaults to the primary text",
  (base["background-image"] ?? "").includes("var(--shimmer-peak, var(--cursor-text-primary))"),
);
check(
  "the loop reads --shimmer-duration with a 2s default",
  (base.animation ?? "").includes("var(--shimmer-duration, 2s)"),
);

const tones = {
  status: {
    base: "color-mix(in srgb, var(--cursor-text-secondary) 60%, transparent)",
    peak: "var(--cursor-text-secondary)",
  },
  thinking: {
    base: "color-mix(in srgb, var(--cursor-text-tertiary) 60%, transparent)",
    peak: "var(--cursor-text-secondary)",
  },
  tool: {
    base: "color-mix(in srgb, var(--cursor-text-secondary) 60%, transparent)",
    peak: "color-mix(in srgb, var(--cursor-text-primary) 60%, transparent)",
  },
};
for (const [tone, expected] of Object.entries(tones)) {
  const rule = declarations(mainCss, `.ws-shimmer-text--${tone}`);
  check(`the ${tone} tone loops in 1s`, rule["--shimmer-duration"] === "1s");
  check(`the ${tone} tone base is ${expected.base}`, rule["--shimmer-base"] === expected.base);
  check(`the ${tone} tone peak is ${expected.peak}`, rule["--shimmer-peak"] === expected.peak);
}

const reduced = declarations(reducedCss, ".ws-shimmer-text");
check("reduced motion stops the animation", reduced.animation === "none");
check("reduced motion drops the gradient", reduced["background-image"] === "none");
check("reduced motion restores the text clip", reduced["background-clip"] === "unset");
check("reduced motion restores the text fill", reduced["-webkit-text-fill-color"] === "unset");
check(
  "reduced motion leaves the row's own resting color",
  reduced.color === undefined,
);

if (failures.length > 0) {
  console.error(`shimmer: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("shimmer: all assertions passed");
