// Title bar under macOS overlay chrome (plan step 22): the desktop app runs the
// window with titleBarStyle Overlay and a hidden title, so the native
// traffic lights float over the bar's left edge and cover
// close/minimize/zoom. window-chrome.ts detects the platform through the
// shared detectPlatform() and, on macOS only, hides the custom
// Windows-style control cluster and marks the bar so window-chrome.css can
// inset the left region clear of the lights. The drag region and the
// maximized/fullscreen sync keep working - the green light's native
// fullscreen is what isFullscreen tracks. Windows and Linux keep the
// cluster; a plain browser changes nothing, because browser mode already
// hides the cluster and no traffic lights overlay the bar.
// Run: node --test test/titlebar-macos.mjs
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));
const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");

const bundle = await esbuild.build({
  entryPoints: [path.join(uiDir, "..", "src", "parts", "chrome", "window-chrome.ts")],
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

// Lets the async maximized/fullscreen sync (stubbed promises) run out.
async function flush() {
  for (let i = 0; i < 5; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

// detectPlatform() reads the global navigator at setup time, so each
// scenario installs the platform's navigator before calling
// setupWindowChrome and restores the original afterward.
const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
const NAVIGATORS = {
  mac: { platform: "MacIntel", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)" },
  windows: { platform: "Win32", userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)" },
  linux: { platform: "Linux x86_64", userAgent: "Mozilla/5.0 (X11; Linux x86_64)" },
};

function scenario({ desktop, platform }) {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/" });
  const { window } = dom;
  if (desktop) {
    window.__TAURI_INTERNALS__ = {};
  }
  globalThis.window = window;
  globalThis.document = window.document;
  globalThis.CustomEvent = window.CustomEvent;
  Object.defineProperty(globalThis, "navigator", {
    value: NAVIGATORS[platform],
    configurable: true,
    writable: true,
  });
  try {
    const chrome = setupWindowChrome();
    return {
      window,
      chrome,
      bar: window.document.querySelector(".ws-window-titlebar"),
      stub: () => window.__TAURI_STUB__,
    };
  } finally {
    Object.defineProperty(globalThis, "navigator", originalNavigator);
  }
}

// --- macOS desktop: the native traffic lights replace the custom cluster ---

{
  const { window, bar, stub } = scenario({ desktop: true, platform: "mac" });
  check("macOS reveals the bar", bar.hidden === false);
  check(
    "macOS hides the custom window-control cluster",
    bar.querySelector(".ws-window-titlebar__controls").hidden === true,
  );
  check(
    "macOS marks the bar for the traffic-light inset",
    bar.classList.contains("ws-window-titlebar--macos"),
  );

  // The empty center still drags the window.
  const drag = bar.querySelector(".ws-window-titlebar__drag");
  drag.dispatchEvent(new window.MouseEvent("mousedown", { button: 0, detail: 1, bubbles: true }));
  check("macOS keeps the drag region live", stub().calls.join(",") === "drag");

  // The maximized/fullscreen sync stays wired: the green light's native
  // fullscreen and the traffic-light zoom still surface as resizes.
  check(
    "macOS keeps the resize sync wired",
    stub().resizeHandlers.length === 1,
  );
  await flush();
  const maximize = bar.querySelector('[data-command="toggle-maximize"]');
  check(
    "macOS still syncs the maximize state",
    maximize.getAttribute("aria-label") === "Maximize",
  );
}

// --- Windows and Linux desktop: the custom cluster stays -------------------

for (const platform of ["windows", "linux"]) {
  const { bar } = scenario({ desktop: true, platform });
  check(
    `${platform} keeps the window-control cluster visible`,
    bar.querySelector(".ws-window-titlebar__controls").hidden === false,
  );
  check(
    `${platform} does not mark the bar for the traffic-light inset`,
    !bar.classList.contains("ws-window-titlebar--macos"),
  );
}

// --- Browser on macOS: nothing changes --------------------------------------

{
  const { bar } = scenario({ desktop: false, platform: "mac" });
  check(
    "a browser on macOS keeps the cluster hidden as plain browser mode",
    bar.querySelector(".ws-window-titlebar__controls").hidden === true,
  );
  check(
    "a browser on macOS takes no traffic-light inset; no lights overlay it",
    !bar.classList.contains("ws-window-titlebar--macos"),
  );
}

if (failures.length > 0) {
  console.error(`titlebar-macos: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("titlebar-macos: all assertions passed");
