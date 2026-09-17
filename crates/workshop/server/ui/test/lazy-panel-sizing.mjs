// Regression test for the lazy panel shell's sizing contract
// (src/ui/layout/panel-types.ts LazyPanel, .ws-panel-lazy in
// src/ui/layout/zones.css, and the agent session's feed/input split in
// src/ui/agent/agent-session.css). Dockview mounts the LazyPanel element
// as the content of a leaf; the real panel swaps in underneath it. Every
// panel root sizes itself with `height: 100%`, so the shell between it
// and dockview's content container must pass the container's height
// through - otherwise the agent feed grows with its transcript instead of
// scrolling, pushes the prompt input below the window, and softlocks the
// panel. jsdom has no layout engine, so the test pins the structural
// contract: the declared styles the cascade assigns to the shell, the
// panel root, and the feed; the feed-then-input DOM order; the autoscroll
// landing on the feed element when a message appends; and LazyPanel
// forwarding dockview's layout(width, height) to the real panel.
// Bundles the modules with esbuild, mounts a real Dockview dock in jsdom
// against the real index.html with the two component stylesheets
// installed as a <style>, and opens the agent session through the zone
// API exactly as main.ts does.
// Run: node test/lazy-panel-sizing.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const uiDir = path.join(testDir, "..");

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import { registerPanelType, registerPanelFactory } from "./src/services/panel-registry.ts";
      // A synthetic lazy panel kind whose factory the test supplies through
      // a global, so the test observes the real panel's layout() calls
      // from outside the bundle.
      registerPanelType({
        type: "sized",
        defaultZone: "main",
        title: "Sized",
        tabComponent: undefined,
        load: () =>
          Promise.resolve({
            register() {
              return registerPanelFactory("sized", () => globalThis.__makeSizedPanel());
            },
          }),
      });
      export { createDockview, themeDark } from "dockview";
      export { initZones, openInZone } from "./src/ui/layout/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/ui/layout/panel-types.ts";
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
  // The modules import their colocated CSS as a side effect; strip it
  // from the bundle. The two sheets under test are installed into the
  // document below, where jsdom's cascade can assign their declarations.
  loader: { ".css": "empty" },
});

const html = await readFile(path.join(uiDir, "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
const { window } = dom;

// The sheets whose declarations size the shell, the panel root, and the
// feed. jsdom resolves declared values through the cascade (it applies
// no layout), so the assertions read the declared contract, not pixels.
const style = window.document.createElement("style");
style.textContent = [
  await readFile(path.join(uiDir, "src", "ui", "layout", "zones.css"), "utf8"),
  await readFile(path.join(uiDir, "src", "ui", "agent", "agent-session.css"), "utf8"),
].join("\n");
window.document.head.appendChild(style);

// The same layout stubs the other dock tests install: jsdom has no layout.
window.matchMedia =
  window.matchMedia ||
  (() => ({
    matches: false,
    media: "",
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  }));
window.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
window.IntersectionObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
};
window.Element.prototype.scrollTo = () => {};
window.HTMLElement.prototype.scrollIntoView = () => {};
window.Range.prototype.getClientRects = () => [];
window.Range.prototype.getBoundingClientRect = () => new window.DOMRect();

for (const key of [
  "document",
  "navigator",
  "location",
  "localStorage",
  "Window",
  "HTMLElement",
  "HTMLTemplateElement",
  "HTMLButtonElement",
  "Node",
  "Element",
  "MutationObserver",
  "Option",
  "DOMParser",
  "ResizeObserver",
  "IntersectionObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
]) {
  if (!(key in globalThis) && key in window) {
    globalThis[key] = window[key];
  }
}
globalThis.Event = window.Event;
globalThis.CustomEvent = window.CustomEvent;
globalThis.window = window;
globalThis.document = window.document;

// The agent panel probes STT capability on mount and the tree lists its
// roots; the probe may fail (the mic stays gated), the roots come back
// empty, and anything else is a regression.
globalThis.fetch = (url) => {
  if (url === "/workspace/tree") {
    return Promise.resolve(
      new Response(JSON.stringify({ path: null, entries: [] }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
  }
  return Promise.reject(new Error(`unexpected fetch in the lazy-panel-sizing test: ${url}`));
};

// A scripted WebSocket for the agent panel's /agents/ws connection: it
// opens on a timer and the test pushes server frames through onmessage.
const sockets = [];
class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  constructor(url) {
    this.url = url;
    this.readyState = FakeWebSocket.CONNECTING;
    this.sent = [];
    sockets.push(this);
    setTimeout(() => {
      this.readyState = FakeWebSocket.OPEN;
      this.onopen?.();
    }, 0);
  }
  addEventListener(type, listener) {
    this[`on${type}`] = listener;
  }
  send(data) {
    this.sent.push(data);
  }
  close() {
    this.readyState = FakeWebSocket.CLOSED;
  }
}
globalThis.WebSocket = FakeWebSocket;

const bundlePath = path.join(os.tmpdir(), "lazy-panel-sizing-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { createDockview, themeDark, initZones, openInZone, createPanelComponent, createPanelTabComponent } =
  await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

async function flush() {
  for (let i = 0; i < 10; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const computed = (element) => window.getComputedStyle(element);

// --- The agent session mounts through the lazy shell into the dock ----------

window.localStorage.clear();
const dock = createDockview(window.document.getElementById("dock"), {
  createComponent: createPanelComponent,
  createTabComponent: createPanelTabComponent,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
initZones(dock);
openInZone("agent", {});
await flush();

const panelRoot = window.document.querySelector("#dock .ws-agent-panel");
check("the agent panel mounted into the dock", panelRoot !== null);
const shell = panelRoot?.parentElement ?? null;
check("the agent panel mounts inside the lazy shell", shell?.classList.contains("ws-panel-lazy") === true);
check(
  "the lazy shell is a direct child of dockview's content container",
  shell?.parentElement?.classList.contains("dv-content-container") === true,
);

// The sizing chain: the shell hands the container's height through, the
// panel root fills the shell, the feed is the flexible scroll region.
if (shell !== null) {
  const shellStyle = computed(shell);
  check("the lazy shell is full height", shellStyle.height === "100%");
  check(
    "the lazy shell is a flex column",
    shellStyle.display === "flex" && shellStyle.flexDirection === "column",
  );
  check("the lazy shell may shrink below its content", shellStyle.minHeight === "0px");
}
if (panelRoot !== null) {
  const rootStyle = computed(panelRoot);
  check("the agent panel root is full height", rootStyle.height === "100%");
  check(
    "the agent panel root is a flex column",
    rootStyle.display === "flex" && rootStyle.flexDirection === "column",
  );
}

const session = panelRoot?.querySelector(".ws-agent-session") ?? null;
const feed = session?.querySelector(".ws-agent-session__feed") ?? null;
const outer = session?.querySelector(".ws-agent-session__outer") ?? null;
check("the session view holds a feed and an input affordance", feed !== null && outer !== null);
if (session !== null && feed !== null && outer !== null) {
  const sessionStyle = computed(session);
  check(
    "the session view is a flex column that fills the panel",
    sessionStyle.display === "flex" &&
      sessionStyle.flexDirection === "column" &&
      sessionStyle.flexGrow === "1" &&
      sessionStyle.minHeight === "0px",
  );
  const feedStyle = computed(feed);
  check(
    "the feed is the flexible region",
    feedStyle.flexGrow === "1" && feedStyle.minHeight === "0px",
  );
  check("the feed is the scroll container", feedStyle.overflowY === "auto");
  check("the input affordance does not flex", computed(outer).flexGrow === "0");
  check(
    "the input affordance follows the feed",
    feed.parentElement === session &&
      outer.parentElement === session &&
      feed.compareDocumentPosition(outer) & window.Node.DOCUMENT_POSITION_FOLLOWING,
  );
}

// --- Appending a message autoscrolls the feed element -------------------------

if (feed !== null) {
  // jsdom reports every scroll metric as 0; a scripted scrollHeight and a
  // spied scrollTop setter pin which element the view scrolls, and to what.
  const scrollTops = [];
  Object.defineProperty(feed, "scrollHeight", { configurable: true, get: () => 4321 });
  Object.defineProperty(feed, "scrollTop", {
    configurable: true,
    get: () => scrollTops.at(-1) ?? 0,
    set: (value) => scrollTops.push(value),
  });

  const agentsSocket = sockets.find((socket) => socket.url.endsWith("/agents/ws"));
  check("the agent panel opened its session socket", agentsSocket !== undefined);
  await flush();
  const push = (frame) => agentsSocket?.onmessage?.({ data: JSON.stringify(frame) });
  push({ type: "agents", agents: ["chat"] });
  push({ type: "agent_session", session: "s1", agent: "chat" });
  push({
    type: "agent_event",
    index: 0,
    event: { kind: "user_message", section: "chat", chain_id: 0, depth: 0, turn: 0, content: "hello" },
  });
  await flush();

  check("the message painted a row in the feed", feed.querySelectorAll(".ws-agent-item").length === 1);
  check(
    "appending a message scrolls the feed to its bottom",
    scrollTops.length > 0 && scrollTops.at(-1) === 4321,
  );
}

// --- LazyPanel forwards layout() to the real panel -----------------------------

const layouts = [];
globalThis.__makeSizedPanel = () => ({
  element: Object.assign(window.document.createElement("div"), { className: "sized-panel" }),
  init() {},
  layout(width, height) {
    layouts.push([width, height]);
  },
  dispose() {},
});

const lazy = createPanelComponent({ id: "sized", name: "sized" });
check("the lazy shell carries its sizing class", lazy.element.className === "ws-panel-lazy");
check("the lazy shell implements layout", typeof lazy.layout === "function");
// A resize before the chunk resolves replays at the swap.
lazy.layout?.(640, 480);
lazy.init({ params: {}, api: {} });
await flush();
check("the real panel swapped in", lazy.element.querySelector(".sized-panel") !== null);
check(
  "a layout before the swap reaches the real panel at the swap",
  layouts.length === 1 && layouts[0][0] === 640 && layouts[0][1] === 480,
);
lazy.layout?.(800, 600);
check(
  "a layout after the swap forwards to the real panel",
  layouts.length === 2 && layouts[1][0] === 800 && layouts[1][1] === 600,
);
lazy.dispose();

if (failures.length > 0) {
  console.error(`lazy-panel-sizing: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("lazy-panel-sizing: all assertions passed");
process.exit(0);
