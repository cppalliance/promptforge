// Lifecycle test for the Agent panel controller
// (src/ui/workshop/agent-controller.ts). Bundles the controller with a real
// Dockview dock in jsdom against the real index.html, but stubs ChatUI
// (an esbuild plugin intercepts src/chat/main) so the test observes the
// controller's own behavior: one chat mounted per Agent tab onto that
// panel's .mur-app surface, the plugins factory running once per tab,
// the shared model applied at mount and broadcast to every live engine,
// active-agent tracking, destroy-on-close with survivor fallback,
// non-agent panels mounting nothing, newChat's removal (New Agent is the
// only new-conversation command), and ensureAgent's guarantee.
// Run: node test/agent-controller.mjs
import { readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

// The controller under test talks to ChatUI only through its constructor,
// engine.setRequestDefaults, and destroy; the stub records all three so
// the assertions stay on the controller.
const stubChatMain = {
  name: "stub-chat-main",
  setup(build) {
    build.onResolve({ filter: /(^|\/)chat\/main(\.ts)?$/ }, () => ({
      path: "chat-main-stub",
      namespace: "chatstub",
    }));
    build.onLoad({ filter: /.*/, namespace: "chatstub" }, () => ({
      contents: `
        export class ChatUI {
          static instances = [];
          constructor(options) {
            this.options = options;
            this.plugins = typeof options.plugins === "function" ? options.plugins() : [];
            this.destroyed = false;
            this.engine = {
              defaults: [],
              setRequestDefaults(d) { this.defaults.push(d); },
            };
            ChatUI.instances.push(this);
          }
          async destroy() { this.destroyed = true; }
        }
      `,
      loader: "js",
    }));
  },
};

const bundle = await esbuild.build({
  stdin: {
    contents: `
      export { createDockview, themeDark } from "dockview";
      export { AgentController } from "./src/ui/workshop/agent-controller.ts";
      export { ModelService } from "./src/services/model-service.ts";
      export { initZones, openAgentPanel, openInZone } from "./src/ui/workshop/zones.ts";
      export { createPanelComponent, createPanelTabComponent } from "./src/ui/workshop/panel-types.ts";
      export { ChatUI } from "./src/chat/main.ts";
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
  // The modules under test import their colocated CSS; strip it - the
  // test drives only the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  plugins: [stubChatMain],
});

const html = await readFile(path.join(uiDir, "..", "index.html"), "utf8");
const dom = new JSDOM(html, { url: "http://127.0.0.1:7910/", pretendToBeVisual: true });
const { window } = dom;

// The same layout stubs the other workshop tests install: jsdom has no layout.
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

// The tree panel fetches its roots on mount; grant it an empty listing
// and reject anything else loudly.
globalThis.fetch = async (url) => {
  if (typeof url === "string" && url.startsWith("/workspace/tree")) {
    return { ok: true, status: 200, json: async () => ({ path: null, entries: [] }) };
  }
  throw new Error(`unexpected fetch in the agent-controller test: ${url}`);
};

for (const key of [
  "document",
  "navigator",
  "location",
  "localStorage",
  "Window",
  "HTMLElement",
  "HTMLTemplateElement",
  "Node",
  "Element",
  "Event",
  "CustomEvent",
  "MutationObserver",
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

// The bundle includes all of dockview, so import from a temp file rather
// than a data URL.
const bundlePath = path.join(os.tmpdir(), "promptforge-agent-controller-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const {
  createDockview,
  themeDark,
  AgentController,
  ModelService,
  initZones,
  openAgentPanel,
  openInZone,
  createPanelComponent,
  createPanelTabComponent,
  ChatUI,
} = await import(pathToFileURL(bundlePath).href);

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

// The dock, wired exactly as main.ts wires it.
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

let pluginBuilds = 0;
const models = new ModelService();
models.setCurrent("model-a");
const agents = new AgentController({
  dock,
  provider: {},
  plugins: () => {
    pluginBuilds += 1;
    return [{ name: `plugin-${pluginBuilds}` }];
  },
  models,
});

const lastModel = (chat) => chat.engine.defaults[chat.engine.defaults.length - 1]?.options?.model;

// --- Mount: one ChatUI per Agent tab ----------------------------------------

const panelA = openAgentPanel();
const chatA = ChatUI.instances[0];
check("one ChatUI mounts per Agent tab", ChatUI.instances.length === 1);
check(
  "the chat mounts onto its own panel's .mur-app surface",
  chatA.options.container instanceof window.HTMLElement &&
    chatA.options.container === panelA.view.content.element.querySelector(".mur-app"),
);
check("the plugins factory runs once per tab", pluginBuilds === 1 && chatA.plugins.length === 1);
check("a new agent receives the shared model selection", lastModel(chatA) === "model-a");
check("the first agent becomes active", agents.active() === chatA);

const panelB = openAgentPanel();
const chatB = ChatUI.instances[1];
check("a second tab mounts a second ChatUI", ChatUI.instances.length === 2 && pluginBuilds === 2);
check("each tab gets isolated plugin state", chatA.plugins[0] !== chatB.plugins[0]);
check("opening a tab makes it the active agent", agents.active() === chatB);
check("a second tab leaves the first agent live", !chatA.destroyed);

// --- Shared model broadcast ---------------------------------------------------

// Through the service, not applyModel directly: this covers the
// controller's onDidChangeCurrent subscription.
models.setCurrent("model-b");
check(
  "a model change broadcasts to every live engine",
  lastModel(chatA) === "model-b" && lastModel(chatB) === "model-b",
);

// --- Active-agent routing -----------------------------------------------------

panelA.api.setActive();
check("activating a tab retargets the active agent", agents.active() === chatA);
check("newChat is gone: New Agent is the only new-conversation command",
  !("newChat" in agents) && typeof agents.newAgent === "function");

// --- Destroy symmetry -----------------------------------------------------------

dock.removePanel(panelA);
check("closing a tab destroys its ChatUI", chatA.destroyed);
check(
  "closing the active tab falls back to a surviving agent",
  agents.active() === chatB && !chatB.destroyed,
);

openInZone("tree", {});
check("non-agent panels never mount a ChatUI", ChatUI.instances.length === 2);

dock.removePanel(panelB);
check("closing the last agent destroys it too", chatB.destroyed);
check("closing the last agent clears the active agent", agents.active() === null);

// --- newAgent and ensureAgent with no agent open --------------------------------

agents.newAgent();
const chatC = ChatUI.instances[2];
check(
  "newAgent with no agent open mounts a fresh one",
  ChatUI.instances.length === 3 && agents.active() === chatC,
);

agents.ensureAgent();
check("ensureAgent keeps a live agent", ChatUI.instances.length === 3);

const panelC = dock.panels.find((panel) => panel.id.startsWith("chat:"));
dock.removePanel(panelC);
agents.ensureAgent();
check(
  "ensureAgent opens an agent when none remain",
  ChatUI.instances.length === 4 && agents.active() === ChatUI.instances[3],
);

if (failures.length > 0) {
  console.error(`agent-controller: ${failures.length} failure(s)`);
  for (const name of failures) {
    console.error(`  FAIL: ${name}`);
  }
  process.exit(1);
}
console.log("agent-controller: all checks passed");
