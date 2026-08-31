// Unit test for the custom window title bar (src/ui/window-chrome.ts). Bundles
// the TS module with esbuild - with "@tauri-apps/api/window" aliased to the
// recording stub in test/helpers - imports it via a data URL, and drives it
// against jsdom built from the real index.html. Covers: without
// __TAURI_INTERNALS__ the bar is revealed but the control cluster hides and
// no native call is made; a missing bar throws; in the desktop app each
// control calls its window method; the drag region only drags on the
// primary button; double-click toggles maximize; and the maximized state
// read back on resize switches the glyph and aria-label on transitions
// only, with the listener dying at dispose.
// Run: node test/window-chrome.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "ui", "window-chrome.ts")],
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The module under test imports its colocated CSS; strip it - the test
  // drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
  },
});
const code = bundle.outputFiles[0].text;
const { setupWindowChrome } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Lets the async maximized sync (a stubbed promise) run to completion.
async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// Each scenario gets a fresh jsdom: setupWindowChrome reads the globals and
// attaches listeners to the DOM it finds at call time.
function scenario({ desktop }) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  if (desktop) {
    window.__TAURI_INTERNALS__ = {};
  }
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.CustomEvent = window.CustomEvent;
  const chrome = setupWindowChrome();
  return {
    window,
    chrome,
    bar: window.document.querySelector(".window-titlebar"),
    stub: () => window.__TAURI_STUB__,
  };
}

// --- Browser mode: bar visible for the menus, native controls hidden --------

{
  const { window, bar } = scenario({ desktop: false });
  check("browser mode reveals the bar", bar.hidden === false);
  const controls = bar.querySelector(".window-titlebar__controls");
  check("browser mode hides the window-control cluster", controls.hidden === true);
  check("browser mode never installs the Tauri internals", !("__TAURI_INTERNALS__" in window));
  check("browser mode makes no native call", window.__TAURI_STUB__ === undefined);
}

// --- Missing markup: the module guards the DOM contract ---------------------

{
  const dom = new JSDOM("", { url: "http://127.0.0.1:7910/" });
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  let threw = false;
  try {
    setupWindowChrome();
  } catch {
    threw = true;
  }
  check("a page without the title bar throws", threw);
}

// --- Desktop mode: reveal and native window commands ------------------------

{
  const { window, bar, chrome, stub } = scenario({ desktop: true });
  check("desktop mode reveals the bar", bar.hidden === false);
  check(
    "desktop mode keeps the window-control cluster visible",
    bar.querySelector(".window-titlebar__controls").hidden === false,
  );

  const callsAfterClick = (command) => {
    stub().calls.length = 0;
    bar.querySelector(`[data-command="${command}"]`).click();
    return stub().calls.join(",");
  };
  check("minimize calls the window method", callsAfterClick("minimize") === "minimize");
  check(
    "maximize calls the window method",
    callsAfterClick("toggle-maximize") === "toggle-maximize",
  );
  check("close calls the window method", callsAfterClick("close") === "close");

  const drag = bar.querySelector(".window-titlebar__drag");
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("pointerdown", { button: 0, bubbles: true }));
  check("primary pointerdown in the empty center starts the drag", stub().calls.join(",") === "drag");
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("pointerdown", { button: 2, bubbles: true }));
  check("non-primary pointerdown does not drag", stub().calls.length === 0);
  stub().calls.length = 0;
  drag.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
  check("double-click toggles maximize", stub().calls.join(",") === "toggle-maximize");

  // The glyphs are SVG, so visibility is the hidden *attribute* - an
  // SVGSVGElement has no `hidden` IDL property, and assigning one would
  // only create an inert expando that no stylesheet can see.
  const maximize = bar.querySelector('[data-command="toggle-maximize"]');
  const maximizeGlyph = maximize.querySelector(".window-titlebar__glyph--maximize");
  const restoreGlyph = maximize.querySelector(".window-titlebar__glyph--restore");

  await flush();
  check(
    "boot syncs the maximize glyph from the window state",
    maximize.getAttribute("aria-label") === "Maximize" &&
      !maximizeGlyph.hasAttribute("hidden") &&
      restoreGlyph.hasAttribute("hidden"),
  );

  stub().maximized = true;
  stub().resizeHandlers.forEach((handler) => handler({}));
  await flush();
  check(
    "a resize into maximized switches the label to Restore",
    maximize.getAttribute("aria-label") === "Restore",
  );
  check(
    "a resize into maximized swaps the glyphs",
    maximizeGlyph.hasAttribute("hidden") && !restoreGlyph.hasAttribute("hidden"),
  );

  stub().maximized = false;
  stub().resizeHandlers.forEach((handler) => handler({}));
  await flush();
  check(
    "a resize into restored switches the label back to Maximize",
    maximize.getAttribute("aria-label") === "Maximize",
  );
  check(
    "a resize into restored restores the glyphs",
    !maximizeGlyph.hasAttribute("hidden") && restoreGlyph.hasAttribute("hidden"),
  );

  // The resize listener dies with the chrome: a later resize leaves the
  // control alone.
  chrome.dispose();
  await flush();
  stub().maximized = true;
  stub().resizeHandlers.forEach((handler) => handler({}));
  await flush();
  check(
    "after dispose a resize leaves the control alone",
    maximize.getAttribute("aria-label") === "Maximize" &&
      !maximizeGlyph.hasAttribute("hidden") &&
      restoreGlyph.hasAttribute("hidden"),
  );
}

if (failures.length > 0) {
  console.error(`window-chrome: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("window-chrome: all assertions passed");
