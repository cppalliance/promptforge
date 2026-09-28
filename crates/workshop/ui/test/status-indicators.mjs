// The status bar's indicator slots (parts/status/status-bar.ts behind
// @workshop/platform/status-indicators): indicators render in `order`
// with stable elements, `set` shows exactly one color and null clears
// it, a tooltip updates the title and the accessible label, a decorative
// indicator is hidden from assistive tech, `dispose` removes the element,
// and a duplicate id throws. The bar owns no LED and no recording port:
// every indicator belongs to the feature that registers it. A synthetic
// "probe" indicator runs through every color and back to off.
// Bundles the TS modules with esbuild and drives them against jsdom built
// from the real index.html.
// Run: node --test test/status-indicators.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { StatusBar } from "./src/parts/status/status-bar.ts";
      export { STATUS_INDICATORS } from "@workshop/platform/status-indicators";
    `,
    resolveDir: path.join(uiDir, ".."),
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

const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;
globalThis.Element = window.Element;
globalThis.HTMLElement = window.HTMLElement;
globalThis.Node = window.Node;
globalThis.getComputedStyle = window.getComputedStyle.bind(window);

const bundlePath = path.join(os.tmpdir(), "promptforge-status-indicators-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { StatusBar, STATUS_INDICATORS } = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

const COLORS = ["green", "amber", "red"];
const litColors = (el) => COLORS.filter((color) => el.classList.contains(`status-bar__led--${color}`));
const ids = () =>
  [...window.document.querySelectorAll(".status-bar__indicators > .status-bar__led")].map(
    (el) => el.dataset.indicator,
  );
const byId = (id) => window.document.querySelector(`.status-bar__led[data-indicator="${id}"]`);

check("the platform exports the STATUS_INDICATORS token", typeof STATUS_INDICATORS?.id === "string");

const bar = new StatusBar();
check(`the host registers no indicator of its own (got ${ids().join(",")})`, ids().length === 0);
check("the host exposes no recording port", !("setRecording" in bar));

// Ordering: registration order differs from `order`; the DOM follows `order`.
const late = bar.register({ id: "late", name: "Late indicator", order: 5 });
const early = bar.register({ id: "early", name: "Early indicator", order: 1 });
const middle = bar.register({ id: "middle", name: "Middle indicator", order: 3 });
check(`indicators render in ascending order (got ${ids().join(",")})`, ids().join(",") === "early,middle,late");
const middleEl = byId("middle");
middle.set("green");
early.set("amber");
check("set keeps the indicator's element stable", byId("middle") === middleEl);
check("indicators keep their order after set", ids().join(",") === "early,middle,late");

// Exactly one color, and null clears it.
middle.set("amber");
check("set replaces the previous color", litColors(middleEl).join(",") === "amber");
middle.set(null);
check("set(null) returns the lens to unlit", litColors(middleEl).length === 0);

// The accessible label and tooltip.
check("name becomes the aria-label", middleEl.getAttribute("aria-label") === "Middle indicator");
check("a non-decorative indicator is not aria-hidden", !middleEl.hasAttribute("aria-hidden"));
middle.set("green", "3 tasks running");
check("a tooltip sets title", middleEl.title === "3 tasks running");
check(
  "a tooltip joins the accessible label",
  middleEl.getAttribute("aria-label").startsWith("Middle indicator") &&
    middleEl.getAttribute("aria-label").includes("3 tasks running"),
);
middle.set("green");
check("a set without a tooltip clears title", middleEl.title === "");
check("a set without a tooltip restores the plain label", middleEl.getAttribute("aria-label") === "Middle indicator");

// Decorative indicators.
const deco = bar.register({ id: "deco", name: "Decoration", order: 9, decorative: true });
const decoEl = byId("deco");
check("decorative sets aria-hidden", decoEl.getAttribute("aria-hidden") === "true");
check("decorative carries no aria-label", !decoEl.hasAttribute("aria-label"));
deco.set("green", "tip");
check("a decorative tooltip still sets title", decoEl.title === "tip");
check("a decorative tooltip adds no aria-label", !decoEl.hasAttribute("aria-label"));

// Duplicate ids throw, and the first registration survives.
let threw = false;
try {
  bar.register({ id: "middle", name: "Again", order: 0 });
} catch {
  threw = true;
}
check("a duplicate id throws", threw);
check("a rejected duplicate leaves the original in place", byId("middle") === middleEl && ids().length === 4);

// dispose removes the element; the id is free again afterward.
late.dispose();
check("dispose removes the element", byId("late") === null);
late.dispose();
const again = bar.register({ id: "late", name: "Late again", order: 5 });
check("a disposed id can register again", byId("late") !== null);
again.dispose();
early.dispose();
middle.dispose();
deco.dispose();

// Descending registration: each new slot lands before the leftmost DOM slot
// with a higher order, not the first such slot in registration order.
const five = bar.register({ id: "five", name: "Five", order: 5 });
const three = bar.register({ id: "three", name: "Three", order: 3 });
const one = bar.register({ id: "one", name: "One", order: 1 });
check(`descending registration renders in ascending order (got ${ids().join(",")})`, ids().join(",") === "one,three,five");
five.dispose();
three.dispose();
one.dispose();

// The probe: every color, then off.
const probe = bar.register({ id: "probe", name: "Probe", order: 2 });
const probeEl = byId("probe");
for (const color of COLORS) {
  probe.set(color);
  check(`the probe shows ${color} alone`, litColors(probeEl).join(",") === color);
}
probe.set(null);
check("the probe returns to off", litColors(probeEl).length === 0);
probe.dispose();

// render never touches indicators: no color on any LED from a frame.
const watched = bar.register({ id: "watched", name: "Watched", order: 0 });
bar.render({ type: "status", label: "x", description: "", severity: "info", activity: "generating", busy: false });
check("render no longer lights an indicator", litColors(byId("watched")).length === 0);
watched.dispose();

bar.dispose();

if (failures.length > 0) {
  console.error(`status-indicators: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("status-indicators: all assertions passed");
process.exit(0);
