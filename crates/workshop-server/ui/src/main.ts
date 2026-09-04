import "shared-ui/tokens.css";
import "dockview/dist/styles/dockview.css";

import { createDockview, themeDark } from "dockview";
import { createToastStack } from "shared-ui/toast";

import { DisposableStore, toDisposable } from "./base/lifecycle";
import { ModelService } from "./services/model-service";
import { UpdateService } from "./services/update-service";
import { WorkbenchService } from "./services/workbench-service";
import { WorkshopSocket } from "./services/workshop-socket";
import { setupGatewayConfigBridge } from "./ui/gateway-config-bridge";
import { markdownReady } from "./ui/markdown-render";
import { StatusBar } from "./ui/status-bar";
import { UpdateView } from "./ui/update-view";
import { setupWindowChrome } from "./ui/window-chrome";
import { setupWindowMenus, type ModelMenuService, type ProfileMenuService } from "./ui/window-menu";
import { setupWorkspaceDrops } from "./ui/workspace-drops";
import { restoreZoom } from "./ui/zoom";
import { restoreLayout, startLayoutPersistence } from "./ui/workshop/layout-persistence";
import { createPanelComponent, createPanelTabComponent } from "./ui/workshop/panel-types";
import { installShortcuts, toggleWorkshopPanel } from "./ui/workshop/shortcuts";
import { initZones, openInZone } from "./ui/workshop/zones";

// The root of the ownership tree: every top-level binding registers here,
// so the whole composition tears down with one dispose() call.
const disposables = new DisposableStore();
const disposePage = (): void => disposables.dispose();
window.addEventListener("pagehide", disposePage, { once: true });
disposables.add(toDisposable(() => window.removeEventListener("pagehide", disposePage)));
const suppressNativeContextMenu = (event: MouseEvent): void => event.preventDefault();
document.addEventListener("contextmenu", suppressNativeContextMenu, { capture: true });
disposables.add(
  toDisposable(() =>
    document.removeEventListener("contextmenu", suppressNativeContextMenu, { capture: true }),
  ),
);

// One persistent socket carries the server's downstream JSON - status
// updates the status bar renders as they arrive, catalog pushes, and
// workbench snapshots. Chat rides the agent panel's own /agents/ws
// socket, composed inside the panel. The status bar builds its own
// shell (shared-ui) and appends it as the body's full-width footer.
const statusBar = disposables.add(new StatusBar());
const updates = disposables.add(new UpdateService());
// The shared toast stack carries the update notifications; the workshop
// keeps it clear of the status bar via --toast-inset-block-end.
const toasts = createToastStack();
document.body.append(toasts.element);
disposables.add(toDisposable(() => toasts.element.remove()));
disposables.add(new UpdateView(updates, toasts));
updates.startAutoCheck();
// The custom title bar stays hidden in a plain browser; it only appears
// when the desktop shell sets its initialization flag.
disposables.add(setupWindowChrome());
// Native webview zoom does not persist across sessions, so the stored
// factor is re-applied on every boot.
restoreZoom();
// Native Explorer drops arrive as a typed event from the desktop shell;
// each path becomes a workspace grant. Inert in a plain browser.
disposables.add(setupWorkspaceDrops(statusBar));
// The Gateway Config panel's postMessage bridge: API forwards go through
// the workshop server's key-attaching proxy, and the panel's action
// notifications (apply, revert, download-started) land on the status bar.
disposables.add(setupGatewayConfigBridge({ statusBar }));
const workshopSocket = disposables.add(new WorkshopSocket());

// The model catalog and selection live in the ModelService, not module
// state: the title-bar Model menu receives the service through its
// constructor and observes its change events. Selecting a model is a
// command the socket carries to the server; the selection itself changes
// only when a workbench snapshot arrives.
const modelService = disposables.add(
  new ModelService((id) => workshopSocket.selectModel(id)),
);

// The rest of the server-owned workbench state - profiles, switch
// progress, chat gating - lives in the WorkbenchService, fed from the
// same snapshots. The Model menu's Profiles section reads it below.
const workbenchService = disposables.add(new WorkbenchService());

disposables.add(workshopSocket.onStatus((frame) => statusBar.render(frame)));
// A dropped socket means every in-flight status is stale; the bar returns
// to its reconnecting state until the observer speaks again.
disposables.add(workshopSocket.onDisconnect(() => statusBar.reset()));
workshopSocket.connect();

// Code blocks in the agent feed highlight through Shiki, whose init is
// async; awaiting it here means the first painted message is never the
// degraded unhighlighted render. A failed init must not block the boot -
// the renderer degrades to plain <pre><code> blocks instead.
await markdownReady.catch((error: unknown) => {
  console.error("markdown highlighting failed to initialize; code blocks render unhighlighted:", error);
});

// Panels are created through the workshop registry: each component name
// maps to a factory in panel-types, and openInZone places panels by zone
// affinity (tree left, editors main, the agent session right). The
// workbench is always unlocked: user drags rearrange panels at any time,
// and the zone registry records the placement overrides. Every panel
// renders a normal chip tab (no singleTabMode: a lone tab stretched
// full-width reads as a second title bar and hides that tabs exist at
// all); the Workshop tree's tab comes from the close-button-free renderer.
const dockEl = document.getElementById("dock") as HTMLDivElement;
const dock = createDockview(dockEl, {
  // The status bar rides along so the Workshop tree's workspace actions
  // (add and remove folders) can announce their outcomes; the model
  // service rides along so the agent session's toolbar picker reads the
  // shared catalog and selection.
  createComponent: (options) => createPanelComponent(options, { statusBar, modelService }),
  createTabComponent: createPanelTabComponent,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
disposables.add(dock);
disposables.add(initZones(dock));

// Restore the persisted layout; any failure falls back to the known-good
// default: the tree anchors the left zone first, then the agent session
// opens right, and main stays empty until a document opens. Panels
// re-create through their factories - only identity is stored.
if (!restoreLayout(dock)) {
  const treePanel = openInZone("tree", {});
  treePanel.group.api.setSize({ width: 280 });
  openInZone("agent", {});
}
// The workbench never boots without its anchors: a restored layout that
// lost the Workshop tree (a stale snapshot from before the tree became
// non-closable) or carries no agent-session panel gets them back. Both
// panels are singletons, so re-opening an existing one only focuses it.
openInZone("tree", {});
openInZone("agent", {});
disposables.add(startLayoutPersistence(dock));
disposables.add(installShortcuts(dock));

// The Model menu's Profiles section: a thin view over the workbench
// snapshots the server pushes. Switching is a command on the socket -
// progress and failure arrive back as server status frames, so the only
// local message is for the failure the server can never report: the
// socket is down and nothing went out.
const profileMenu: ProfileMenuService = {
  get profiles() {
    return workbenchService.snapshot.profiles;
  },
  get active() {
    return workbenchService.snapshot.active ?? "";
  },
  get switching() {
    return workbenchService.snapshot.switching ?? "";
  },
  onDidChange: workbenchService.onDidChangeSnapshot,
  switchTo(name: string): void {
    if (!workshopSocket.switchProfile(name)) {
      statusBar.showLocal(`Could not switch to ${name}: the workshop socket is down`, "error");
    }
  },
};

// The Model menu's catalog section: a thin view over the model service.
// Selecting a model is a command on the socket - the confirmed selection
// arrives back in a workbench snapshot - so, exactly as with a profile
// switch above, the only local message is for the failure the server can
// never report: the socket is down and nothing went out.
const modelMenu: ModelMenuService = {
  get models() {
    return modelService.models;
  },
  get current() {
    return modelService.current;
  },
  setCurrent(id: string): void {
    if (!modelService.setCurrent(id)) {
      statusBar.showLocal(`Could not select ${id}: the workshop socket is down`, "error");
    }
  },
};

const openNewAgent = (): void => {
  openInZone("agent", { instance: window.crypto.randomUUID() });
};

// The title-bar menus dispatch through one shared command set; the
// keyboard shortcuts call the same workshop command functions. The Model
// menu reads the model service's catalog and writes the selection back
// into it, and its Profiles section reads the workbench service through
// the profileMenu view above. Each New Agent command creates a separate
// panel, socket, and modal server session in the right zone.
disposables.add(
  setupWindowMenus({
    agents: {
      newAgent: openNewAgent,
    },
    workshop: {
      toggleWorkshopPanel: () => toggleWorkshopPanel(dock),
      openGatewayConfig: () => {
        openInZone("config", {});
      },
      openAgentSession: openNewAgent,
    },
    modelMenu,
    profileMenu,
    updates,
  }),
);

// A pushed catalog means the gateway returned after an outage; refresh the
// catalog state in place so a boot-time failure heals itself.
disposables.add(workshopSocket.onModels((models) => modelService.setModels(models)));

// The server-owned selection and the rest of the workbench state arrive
// in the same snapshot: the model service takes the selection, the
// workbench service the whole frame.
disposables.add(
  workshopSocket.onWorkbench((frame) => {
    modelService.applySelected(frame.selected);
    workbenchService.applySnapshot(frame);
  }),
);

// Every push handler above is wired, so release the socket's boot queue:
// pushes that raced this module's execution now replay in arrival order.
workshopSocket.ready();
