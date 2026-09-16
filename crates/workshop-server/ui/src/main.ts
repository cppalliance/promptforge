import "shared-ui/tokens.css";
import "dockview/dist/styles/dockview.css";

import "./tokens/base.css";
import "./tokens/semantic.css";
import "./tokens/component.css";

import { createDockview, themeDark } from "dockview";
import { createToastStack } from "shared-ui/toast";

import { DisposableStore, toDisposable } from "./base/lifecycle";
import { ModelService, MODEL_SERVICE } from "./services/model-service";
import { getService, registerService } from "./services/service-registry";
import { SpeechCaptureService, SPEECH_CAPTURE } from "./services/speech-capture";
import { TEXT_CONTROL_SERVICE } from "./services/text-control-service";
import { UpdateService } from "./services/update-service";
import { WorkbenchService } from "./services/workbench-service";
import { WorkshopSocket } from "./services/workshop-socket";
import { CommandCenter } from "./ui/chrome/command-center";
import { EDITOR_SETTINGS_SERVICE } from "./ui/editor/editor-settings-service";
import { setupGatewayConfigBridge } from "./ui/gateway/gateway-config-bridge";
import { StatusBar, STATUS_BAR } from "./ui/status/status-bar";
import { UpdateView } from "./ui/chrome/update-view";
import { setupWindowChrome } from "./ui/chrome/window-chrome";
import { setupWindowMenus } from "./ui/menu/index";
import { KeybindingDispatcher } from "./ui/layout/keybinding-dispatcher";
import { QuickInputService, QUICK_INPUT_SERVICE } from "./ui/quickinput/quick-input";
import { setupWorkspaceDrops } from "./ui/workspace/workspace-drops";
import { restoreZoom } from "./ui/chrome/zoom";
import { restoreLayout, startLayoutPersistence } from "./ui/layout/layout-persistence";
import { createPanelComponent, createPanelTabComponent } from "./ui/layout/panel-types";
import { initZones, openInZone } from "./ui/layout/zones";

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
// state: the agent toolbar's picker resolves the service from the
// registry and observes its change events. Selecting a model is a
// command the socket carries to the server; the selection itself changes
// only when a workbench snapshot arrives. The service subscribes itself
// to the socket's catalog push, so a gateway returning after an outage
// heals a boot-time empty catalog in place.
const modelService = disposables.add(
  new ModelService((id) => workshopSocket.selectModel(id), workshopSocket.onModels),
);

// The rest of the server-owned workbench state - profiles, switch
// progress, chat gating - lives in the WorkbenchService, fed from the
// same snapshots.
const workbenchService = disposables.add(new WorkbenchService());
const speechCapture = new SpeechCaptureService();

// The composition root's services register into the service registry;
// the lazy feature directories (the panels) resolve them from there when
// their chunks activate, instead of receiving them through the dock's
// createComponent seam.
registerService(STATUS_BAR, () => statusBar);
registerService(MODEL_SERVICE, () => modelService);
registerService(SPEECH_CAPTURE, () => speechCapture);

// The focus-tracking and editor-settings services resolve at boot so the
// inputFocus/editorTextFocus/textInputFocus and config.editor.* context
// keys exist from first paint; both self-register with default factories,
// and the settings module stays CodeMirror-free so the lazy chunk split
// holds.
disposables.add(getService(TEXT_CONTROL_SERVICE));
disposables.add(getService(EDITOR_SETTINGS_SERVICE));

// The quick input widget: one instance for the page lifetime, registered
// for the quick-access actions (Command Palette, Go to File, Go to Line)
// to resolve at call time. Its panel anchors under the title bar.
const quickInput = disposables.add(new QuickInputService());
registerService(QUICK_INPUT_SERVICE, () => quickInput);

// The command center: the title bar's center drag region hosts the pill
// (search icon, window title, quick-access chevron) as a no-drag child.
const titleCenter = document.querySelector<HTMLElement>(".ws-window-titlebar__center");
if (!titleCenter) {
  throw new Error("DOM Error: .ws-window-titlebar__center not found in the page.");
}
disposables.add(new CommandCenter(titleCenter));

// The lazy directories register through the panel registry when their
// chunks load; every action, menu row, and keybinding registers eagerly
// from the contribution surface the menu bootstrap imports.
disposables.add(workshopSocket.onStatus((frame) => statusBar.render(frame)));
// A dropped socket means every in-flight status is stale; the bar returns
// to its reconnecting state until the observer speaks again.
disposables.add(workshopSocket.onDisconnect(() => statusBar.reset()));
workshopSocket.connect();

// Panels are created through the panel registry: each component name
// maps to a lazy import thunk, and openInZone places panels by zone
// affinity (tree left, editors main, the agent session right). The
// workbench is always unlocked: user drags rearrange panels at any time,
// and the zone registry records the placement overrides. Every panel
// renders a normal chip tab (no singleTabMode: a lone tab stretched
// full-width reads as a second title bar and hides that tabs exist at
// all); the Workshop tree's tab comes from the close-button-free renderer.
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
disposables.add(dock);
disposables.add(speechCapture);
disposables.add(initZones(dock));

// Restore the persisted layout; any failure falls back to the known-good
// default: the tree anchors the left zone first, then the agent session
// opens right, and main stays empty until a document opens. Panels
// re-create through their registered factories - only identity is stored.
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
// The keybinding dispatcher owns every registered chord: one
// capture-phase listener resolving through the keybinding registry the
// contributions populate.
disposables.add(new KeybindingDispatcher());

// The title-bar menus dispatch through the command and menu registries
// the contribution surface populates (imported by the menu bootstrap at
// module scope); the keyboard chords reach the same commands through the
// dispatcher. The bar generates its buttons from the MenubarMainMenu
// submenu rows.
disposables.add(setupWindowMenus());

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
