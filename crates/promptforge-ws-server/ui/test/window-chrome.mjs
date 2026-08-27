// Unit test for the custom window title bar (src/ui/window-chrome.ts). Bundles
// the TS module with esbuild, imports it via a data URL, and drives it
// against jsdom built from the real index.html. Covers: without the desktop
// flag the bar is revealed but the control cluster hides and ipc stays
// untouched; a missing bar throws; under the flag the bar is revealed and
// each control posts its typed command envelope; the drag region only drags
// on the primary button; double-click toggles maximize; and the
// promptforge:maximized event switches the glyph and aria-label while
// malformed details are ignored.
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
});
const code = bundle.outputFiles[0].text;
const { setupWindowChrome } = await import(
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// Each scenario gets a fresh jsdom: setupWindowChrome reads the globals and
// attaches listeners to the DOM it finds at call time.
function scenario({ desktop }) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  const posted = [];
  if (desktop) {
    window.__PROMPTFORGE_DESKTOP__ = true;
    window.ipc = { postMessage: (message) => posted.push(JSON.parse(message)) };
  }
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.CustomEvent = window.CustomEvent;
  setupWindowChrome();
  return { window, bar: window.document.querySelector(".window-titlebar"), posted };
}

// --- Browser mode: bar visible for the menus, native controls hidden --------

{
  const { window, bar } = scenario({ desktop: false });
  check("browser mode reveals the bar", bar.hidden === false);
  const controls = bar.querySelector(".window-titlebar__controls");
  check("browser mode hides the window-control cluster", controls.hidden === true);
  check("browser mode never installs window.ipc", !("ipc" in window));
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

// --- Desktop mode: reveal and typed commands --------------------------------

{
  const { window, bar, posted } = scenario({ desktop: true });
  check("desktop mode reveals the bar", bar.hidden === false);
  check(
    "desktop mode keeps the window-control cluster visible",
    bar.querySelector(".window-titlebar__controls").hidden === false,
  );

  const commandsAfterClick = (command) => {
    posted.length = 0;
    bar.querySelector(`[data-command="${command}"]`).click();
    return posted.map((message) => message.command).join(",");
  };
  check("minimize posts its command", commandsAfterClick("minimize") === "minimize");
  check(
    "maximize posts its command",
    commandsAfterClick("toggle-maximize") === "toggle-maximize",
  );
  check("close posts its command", commandsAfterClick("close") === "close");

  const drag = bar.querySelector(".window-titlebar__drag");
  posted.length = 0;
  drag.dispatchEvent(new window.MouseEvent("pointerdown", { button: 0, bubbles: true }));
  check(
    "primary pointerdown in the empty center starts the drag",
    posted.map((message) => message.command).join(",") === "drag",
  );
  posted.length = 0;
  drag.dispatchEvent(new window.MouseEvent("pointerdown", { button: 2, bubbles: true }));
  check("non-primary pointerdown does not drag", posted.length === 0);
  posted.length = 0;
  drag.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
  check(
    "double-click toggles maximize",
    posted.map((message) => message.command).join(",") === "toggle-maximize",
  );

  const maximize = bar.querySelector('[data-command="toggle-maximize"]');
  const maximizeGlyph = maximize.querySelector(".window-titlebar__glyph--maximize");
  const restoreGlyph = maximize.querySelector(".window-titlebar__glyph--restore");

  window.dispatchEvent(
    new window.CustomEvent("promptforge:maximized", { detail: { maximized: true } }),
  );
  check(
    "maximized event switches the label to Restore",
    maximize.getAttribute("aria-label") === "Restore",
  );
  check("maximized event hides the maximize glyph", maximizeGlyph.hidden === true);
  check("maximized event shows the restore glyph", restoreGlyph.hidden === false);

  window.dispatchEvent(
    new window.CustomEvent("promptforge:maximized", { detail: { maximized: false } }),
  );
  check(
    "restore event switches the label back to Maximize",
    maximize.getAttribute("aria-label") === "Maximize",
  );
  check(
    "restore event restores the glyphs",
    maximizeGlyph.hidden === false && restoreGlyph.hidden === true,
  );

  window.dispatchEvent(
    new window.CustomEvent("promptforge:maximized", { detail: { maximized: "yes" } }),
  );
  window.dispatchEvent(new window.CustomEvent("promptforge:maximized", { detail: null }));
  window.dispatchEvent(new window.Event("promptforge:maximized"));
  check(
    "malformed maximized events leave the control alone",
    maximize.getAttribute("aria-label") === "Maximize" &&
      maximizeGlyph.hidden === false &&
      restoreGlyph.hidden === true,
  );
}

if (failures.length > 0) {
  console.error(`window-chrome: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("window-chrome: all assertions passed");
