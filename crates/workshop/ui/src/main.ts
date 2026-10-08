import "@workshop/look/tokens.css";
import "@workshop/look/controls.css";
import "@workshop/look/shimmer.css";
import "dockview/dist/styles/dockview.css";

import "@workshop/look/sizes.css";
import "@workshop/look/semantic.css";
import "./tokens/component.css";

import { createDockview, themeDark } from "dockview";
import { createToastStack } from "@workshop/look/toast";

import { DisposableStore, toDisposable } from "@workshop/platform/lifecycle";
import { ModelService, MODEL_SERVICE } from "./services/model-service";
import { CLOSED_EDITORS } from "./services/closed-editors";
import { COMMANDS_HISTORY } from "./services/commands-history";
import { EDITOR_SETTINGS_SERVICE } from "./services/editor-settings-service";
import { LAYOUT_POLICY } from "./services/layout-policy";
import { QUICK_INPUT_SERVICE } from "./services/quick-input-service";
import { STATUS_BAR } from "@workshop/platform/status-bar";
import { STATUS_INDICATORS } from "@workshop/platform/status-indicators";
import { RECENT_FILES_STORE, RecentFilesStore } from "./services/recent-files-store";
import { getService, registerService } from "@workshop/platform/service-registry";
import { SpeechCaptureService, SPEECH_CAPTURE } from "./services/speech-capture";
import { STT_STATUS, type SttStatus } from "./services/stt-status";
import { TEXT_CONTROL_SERVICE } from "@workshop/platform/text-control-service";
import { TOAST_STACK } from "./services/toast-service";
import { TREE_STATE, TreeStateService } from "./services/tree-state-service";
import { createUiStorage, UI_STORAGE } from "./services/ui-storage";
import { UpdateService } from "./services/update-service";
import { WorkbenchService } from "./services/workbench-service";
import { WorkshopSocket } from "./services/workshop-socket";
import { CommandCenter } from "./parts/chrome/command-center";
import { ClosedEditors } from "./parts/editor/closed-editors";
import { EditorSettingsService } from "./parts/editor/editor-settings-service";
import { setupGatewayConfigBridge } from "./parts/gateway/gateway-config-bridge";
import { ActivityIndicator } from "./parts/status/activity-indicator";
import { MicMeter } from "./parts/status/mic-meter";
import { StatusBar } from "./parts/status/status-bar";
import { UpdateView } from "./parts/chrome/update-view";
import { setupWindowChrome } from "./parts/chrome/window-chrome";
// Panel types register through this import's contribution modules, which
// must evaluate before the layout boots: keep it static.
import { setupWindowMenus } from "./parts/menu/index";
import { KeybindingDispatcher } from "./parts/layout/keybinding-dispatcher";
import { CommandsHistory } from "./parts/quickinput/commands-history";
import { QuickInputService } from "./parts/quickinput/quick-input";
import { setupWorkspaceDrops } from "./parts/workspace/workspace-drops";
import { register as registerWorkspaceDocument } from "./parts/workspace-document/index";
import { persistZoom, restoreZoom } from "./parts/chrome/zoom";
import { applyLayoutOrDefault } from "./parts/layout/layout-boot";
import { startLayoutPersistence } from "./parts/layout/layout-persistence";
import { createPanelComponent, createPanelTabComponent, PANEL_TAB } from "./parts/layout/panel-types";
import { createAgentPaneHeader } from "./parts/agent/agent-pane-header";
import { bindActiveEditorKey, initZones, openInZone } from "./parts/layout/zones";

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

// The UI-state adapter: the server-backed replacement for browser storage,
// which the per-launch loopback port made session-scoped. Both buckets
// (the user's ui-state.json, the open workspace file) preload here, ahead
// of every service resolution and the dock, so each store reads its
// initial value synchronously from the cache when it is built. The
// preload never rejects: a bucket that fails or hangs past the timeout
// reads as empty with one warning, and boot continues on defaults, so a
// slow or dead server delays the page by at most the timeout.
const storage = createUiStorage();
await storage.preload(3000);
registerService(UI_STORAGE, () => storage);

// The user-scoped stores rebind to the live adapter here, ahead of their
// first resolution: each is built from its user-bucket value and writes
// every change back to the same key. The import-time default factories
// (defaults, no-op writer) stay behind only for a consumer that resolves
// a token before this line, which none does at boot.
registerService(
  EDITOR_SETTINGS_SERVICE,
  () =>
    new EditorSettingsService(storage.get("user", "editor_settings"), (value) =>
      storage.set("user", "editor_settings", value),
    ),
);
registerService(
  RECENT_FILES_STORE,
  () =>
    new RecentFilesStore(storage.get("user", "recent_files"), (value) =>
      storage.set("user", "recent_files", value),
    ),
);
registerService(
  COMMANDS_HISTORY,
  () =>
    new CommandsHistory(storage.get("user", "commands_history"), (value) =>
      storage.set("user", "commands_history", value),
    ),
);
// The tree's expanded folders belong to the workspace: they seed from
// the open file's "tree" value and write back to the same key, debounced.
registerService(
  TREE_STATE,
  () =>
    new TreeStateService(storage.get("workspace", "tree"), (value) =>
      storage.set("workspace", "tree", value),
    ),
);
// The closed-editor stack belongs to the workspace too: it seeds from the
// file's "closed_editors" value and writes back on every close and
// reopen. Bound here, ahead of the dock, so the editor chunk's tracking
// (installed when the chunk first loads) resolves the live-bound stack.
registerService(
  CLOSED_EDITORS,
  () =>
    new ClosedEditors(storage.get("workspace", "closed_editors"), (value) =>
      storage.set("workspace", "closed_editors", value),
    ),
);

// One persistent socket delivers the server's downstream JSON - status
// updates the status bar renders as they arrive, catalog pushes, and
// workbench snapshots. Chat goes over the agent panel's own /agents/ws
// socket, composed inside the panel. The status bar builds its own
// view (@workshop/look) and appends it as the body's full-width footer.
const statusBar = disposables.add(new StatusBar());
const updates = disposables.add(new UpdateService());
// The shared toast stack shows the update notifications and the views'
// confirmations (a copied message); it registers so those views reach it
// without importing the app. The workshop keeps it clear of the status
// bar via --toast-inset-block-end.
const toasts = createToastStack();
document.body.append(toasts.element);
disposables.add(toDisposable(() => toasts.element.remove()));
registerService(TOAST_STACK, () => toasts);
disposables.add(new UpdateView(updates, toasts));
updates.startAutoCheck();
// The custom title bar stays hidden in a plain browser; it only appears
// when the desktop app sets its initialization flag.
disposables.add(setupWindowChrome());
// Native webview zoom does not persist across sessions, so the stored
// factor is re-applied on every boot from the user bucket; the writer
// installs after the restore so the restore never echoes the factor back.
restoreZoom(storage.get("user", "zoom"));
disposables.add(persistZoom((value) => storage.set("user", "zoom", value)));
// Native Explorer drops arrive as a typed event from the desktop app;
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
// command the socket sends to the server; the selection itself changes
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
registerService(STATUS_INDICATORS, () => statusBar);
// The recording LED is the product's indicator, and dictation reaches it
// and the bar's text through one port. Both register here, before the
// layout restores: a restored agent panel resolving no port would fall
// back to a silent one and the LED would die without an error.
const recording = disposables.add(
  statusBar.register({ id: "recording", name: "Recording indicator", order: 0 }),
);
const sttStatus: SttStatus = {
  showLocal: (label, severity) => statusBar.showLocal(label, severity),
  setRecording: (on) => recording.set(on ? "red" : null),
};
registerService(STT_STATUS, () => sttStatus);
registerService(MODEL_SERVICE, () => modelService);
registerService(SPEECH_CAPTURE, () => speechCapture);
// The mic meter stands left of the recording LED and follows the same
// shared capture the dictation surfaces take turns owning.
disposables.add(new MicMeter(statusBar, speechCapture));

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

// The command center: the title bar's center drag region holds the
// folder-name button (the window title, opening quick open) as a no-drag child.
const titleCenter = document.querySelector<HTMLElement>(".ws-window-titlebar__center");
if (!titleCenter) {
  throw new Error("DOM Error: .ws-window-titlebar__center not found in the page.");
}
disposables.add(new CommandCenter(titleCenter));

// The lazy directories register through the panel registry when their
// chunks load; every action, menu row, and keybinding registers eagerly
// from the contribution surface the menu bootstrap imports.
// The activity LED is the product's indicator: the status frame type is
// product protocol, so the bar holds the slot and this part fills it.
const activityIndicator = disposables.add(new ActivityIndicator(statusBar));
disposables.add(
  workshopSocket.onStatus((frame) => {
    statusBar.render(frame);
    activityIndicator.render(frame);
  }),
);
// A dropped socket means every in-flight status is stale; the bar returns
// to its reconnecting state until the observer speaks again.
disposables.add(
  workshopSocket.onDisconnect(() => {
    statusBar.reset();
    activityIndicator.reset();
  }),
);
workshopSocket.connect();

// The default layout is product policy, not layout mechanics: the tree
// opens left and the agent session right, main stays empty until a
// document opens, and both anchors come back whenever a restored layout
// lost one. Registered before the dock boots, because the boot's layout
// apply resolves it.
registerService(LAYOUT_POLICY, () => ({
  anchors: ["tree", "agent"],
  seed: () => {
    const tree = openInZone("tree", {});
    openInZone("agent", {});
    // A lone group always fills the dock, so the tree takes its width only
    // once the agent's group shares the row. (The chat pane's own preference,
    // min(400px, W/4) and never under 300px, applies when its group is
    // created beside others; here the tree's 280px leaves it the rest.)
    tree.group.api.setSize({ width: 280 });
  },
}));

// Panels are created through the panel registry: each component name
// maps to a lazy import thunk, and openInZone places panels by zone
// affinity (tree left, editors main, the agent session right). The
// workbench is always unlocked: user drags rearrange panels at any time,
// and the zone registry records the placement overrides. Every panel
// renders the generic chip tab (no singleTabMode: a lone tab stretched
// full-width reads as a second title bar and hides that tabs exist at
// all), which drops the close button and tab menu for a type registered
// closable: false, the Workshop tree.
const dockEl = document.getElementById("dock") as HTMLDivElement;
const dock = createDockview(dockEl, {
  createComponent: createPanelComponent,
  createTabComponent: createPanelTabComponent,
  // The right zone's New Agent, More Actions, and Close buttons; the
  // stylesheet shows them for that zone's groups only.
  createRightHeaderActionComponent: createAgentPaneHeader,
  defaultTabComponent: PANEL_TAB,
  theme: themeDark,
  disableFloatingGroups: true,
  hideBorders: true,
  locked: false,
  noPanelsOverlay: "emptyGroup",
});
disposables.add(dock);
disposables.add(speechCapture);
disposables.add(initZones(dock));
disposables.add(bindActiveEditorKey(dock));
// Dockview first sizes itself from a resize callback that lands after this
// module runs, and the default layout's pixel widths hold only against a
// sized dock, so the dock takes its element's size before the boot apply.
dock.layout(dockEl.clientWidth, dockEl.clientHeight);

// The dock layout belongs to the workspace: the preloaded workspace
// bucket's "layout" value restores, or any failure falls back to the
// layout policy's default (layout-boot.ts). The debounced saver installs after
// the boot layout is in place, so the restore never echoes the same
// envelope back to the file.
applyLayoutOrDefault(dock, storage.get("workspace", "layout"));
disposables.add(startLayoutPersistence(dock, (value) => storage.set("workspace", "layout", value)));
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
// The workspace-document actions (Open Workspace from File...) register
// eagerly with the contribution surface above; the feature's activation
// hands their registrations to the ownership tree.
disposables.add(registerWorkspaceDocument());

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
