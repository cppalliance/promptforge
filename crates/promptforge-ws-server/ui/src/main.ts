// murm-ui's own styles, bundled by esbuild into dist/app.css. Sidebar and
// dropdown styles are skipped: the workshop disables the murm sidebar and
// no plugin renders dropdowns.
import "./chat/styles/base.css";
import "./chat/styles/feed.css";
import "./chat/styles/input.css";
import "dockview/dist/styles/dockview.css";

import { createDockview, themeDark } from "dockview";

import { DisposableStore } from "./base/lifecycle";
import type { ChatPlugin } from "./chat/core/types";
import { ThinkingPlugin } from "./chat/plugins/thinking/thinking-plugin";
import { ToolsPlugin } from "./chat/plugins/tools/tools-plugin";
import { ModelService } from "./services/model-service";
import type { CatalogModel } from "./services/protocol";
import { WorkshopProvider } from "./services/workshop-provider";
import { WorkshopSocket } from "./services/workshop-socket";
import { StatusBar } from "./ui/status-bar";
import { setupVoice, voiceGpuAvailable, type VoiceHandle } from "./ui/voice";
import { setupWindowChrome } from "./ui/window-chrome";
import { setupWindowMenus } from "./ui/window-menu";
import { setupWorkspaceDrops } from "./ui/workspace-drops";
import { AgentController } from "./ui/workshop/agent-controller";
import { restoreLayout, startLayoutPersistence } from "./ui/workshop/layout-persistence";
import { createPanelComponent, createPanelTabComponent } from "./ui/workshop/panel-types";
import { installShortcuts, toggleWorkshopPanel } from "./ui/workshop/shortcuts";
import { initZones, openInZone } from "./ui/workshop/zones";

// The root of the ownership tree: every top-level binding registers here,
// so the whole composition tears down with one dispose() call.
const disposables = new DisposableStore();

// The model catalog and selection live in the ModelService, not module
// state: the title-bar Model menu and the Agent controller receive the
// service through their constructors and observe its change events.
const modelService = disposables.add(new ModelService());

// One persistent socket carries chat frames upstream and every downstream
// JSON frame - chat replies and the observer's status updates, which the
// status bar renders as they arrive.
const statusBarRoot = document.querySelector(".status-bar") as HTMLElement | null;
if (!statusBarRoot) {
  throw new Error("DOM Error: .status-bar not found in the page.");
}
const statusBar = disposables.add(new StatusBar(statusBarRoot));
// The custom title bar stays hidden in a plain browser; it only appears
// when the desktop shell sets its initialization flag.
disposables.add(setupWindowChrome());
// Native Explorer drops arrive as a typed event from the desktop shell;
// each path becomes a workspace grant. Inert in a plain browser.
disposables.add(setupWorkspaceDrops(statusBar));
const workshopSocket = disposables.add(new WorkshopSocket());
disposables.add(workshopSocket.onStatus((frame) => statusBar.render(frame)));
// A dropped socket means every in-flight status is stale; the bar returns
// to its reconnecting state until the observer speaks again.
disposables.add(workshopSocket.onDisconnect(() => statusBar.reset()));
// An aborted chat recycles the socket, so no terminal status frame for it
// ever arrives; the bar clears its own activity LED instead.
disposables.add(workshopSocket.onAbort(() => statusBar.clearActivity()));
workshopSocket.connect();

// The mic button joins murm-ui's composer through the plugin seam, but only
// when the server can transcribe on a GPU; a CPU take stalls long enough to
// read as broken, so the control stays hidden instead. Voice messages paint
// the status bar directly. Each Agent tab gets its own plugin instance -
// the handle is per-tab so recording in one tab never touches another.
function createVoicePlugin(): ChatPlugin {
  let voiceHandle: VoiceHandle | null = null;
  return {
    name: "voice",
    onInputMount({ form, input }) {
      void voiceGpuAvailable().then((gpu) => {
        if (!gpu) {
          return;
        }
        const mic = document.createElement("button");
        mic.type = "button";
        mic.className = "voice-mic mur-form-icon-btn";
        mic.title = "Push to talk";
        mic.setAttribute("aria-label", "Push to talk");
        mic.setAttribute("aria-pressed", "false");
        mic.innerHTML =
          '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="2" width="6" height="12" rx="3"></rect><path d="M5 10a7 7 0 0 0 14 0"></path><line x1="12" y1="19" x2="12" y2="22"></line></svg>';
        form.insertBefore(mic, form.querySelector(".mur-form-footer-right"));

        voiceHandle = setupVoice({ mic, input }, statusBar);
      });
    },
    onUserSubmit() {
      voiceHandle?.discardIfRecording();
    },
    // Closing a tab destroys its ChatUI, which fires this hook: the mic
    // unwires and a live take is discarded with the tab that owned it.
    destroy() {
      voiceHandle?.dispose();
      voiceHandle = null;
    },
    // With no model selected there is nothing to send to; the old UI disabled
    // the send button in the same situation.
    isSubmitBlocked: () => !modelService.current,
  };
}

// Panels are created through the workshop registry: each component name
// maps to a factory in panel-types, and openInZone places panels by zone
// affinity (tree left, editors main, chat right). The workbench is always
// unlocked: user drags rearrange panels at any time, and the zone
// registry records the placement overrides. Every panel renders a normal
// chip tab (no singleTabMode: a lone tab stretched full-width reads as a
// second title bar and hides that tabs exist at all); the Workshop tree's
// tab comes from the close-button-free renderer.
const dockEl = document.getElementById("dock") as HTMLDivElement;
const dock = createDockview(dockEl, {
  createComponent: createPanelComponent,
  createTabComponent: createPanelTabComponent,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
// Teardown order matters: the dock registers before the Agent controller,
// so a root dispose() tears the panels down while the controller still
// listens - each closing Agent tab destroys its ChatUI through unmount.
disposables.add(dock);
disposables.add(initZones(dock));

// The Agent controller observes the dock: every Agent panel that appears
// (New Agent, or a restored layout recreating its tabs) gets its own
// ChatUI with isolated session and plugin state, and closing a tab
// destroys only that agent. All agents share one provider - the workshop
// socket multiplexes concurrent chat streams by request id - and the
// model service's shared selection, whose changes the controller
// broadcasts to every live engine. The controller must exist before
// restoreLayout so restored tabs mount their chats.
const agents = disposables.add(
  new AgentController({
    dock,
    provider: new WorkshopProvider(workshopSocket),
    plugins: () => [createVoicePlugin(), ThinkingPlugin(), ToolsPlugin()],
    models: modelService,
  }),
);

// Restore the persisted layout; any failure falls back to the known-good
// default: the tree anchors the left zone first, then one Agent opens
// right, and main stays empty until a document opens. Panels re-create
// through their factories - only identity is stored.
if (!restoreLayout(dock)) {
  const treePanel = openInZone("tree", {});
  treePanel.group.api.setSize({ width: 280 });
  agents.newAgent();
}
// The workbench never boots without its anchors: a restored layout that
// lost the Workshop tree (a stale snapshot from before the tree became
// non-closable) or carries no Agent panel gets them back.
openInZone("tree", {});
agents.ensureAgent();
disposables.add(startLayoutPersistence(dock));
disposables.add(installShortcuts(dock));

// The title-bar menus dispatch through one shared command set; the
// keyboard shortcuts call the same workshop command functions. The Model
// menu reads the model service's catalog and writes the selection back
// into it. File > New Agent opens a fresh tab; Window > Workshop Panel
// shares Ctrl+B's toggle.
disposables.add(
  setupWindowMenus({
    agents,
    workshop: { toggleWorkshopPanel: () => toggleWorkshopPanel(dock) },
    modelMenu: modelService,
  }),
);

async function loadModels(): Promise<void> {
  try {
    const response = await fetch("/v1/models");
    if (!response.ok) {
      throw new Error(`GET /v1/models answered ${response.status}`);
    }
    const catalog = (await response.json()) as { data?: CatalogModel[] };
    modelService.setModels(Array.isArray(catalog.data) ? catalog.data : []);
  } catch (error) {
    // The Model menu shows its empty state until a pushed catalog heals
    // the failed boot fetch.
    console.error("Could not load the model catalog:", error);
  }
}

// A pushed catalog means the gateway returned after an outage; refresh the
// catalog state in place so a boot-time failure heals itself.
disposables.add(workshopSocket.onModels((models) => modelService.setModels(models)));

// Every push handler above is wired, so release the socket's boot queue:
// pushes that raced this module's execution now replay in arrival order.
workshopSocket.ready();

void loadModels();
