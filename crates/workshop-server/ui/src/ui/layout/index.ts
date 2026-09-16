// The layout feature's entry point: register() is the panel registry's
// activation hook. Importers point at the source files directly; this
// module re-exports nothing.

import { DisposableStore, type IDisposable } from "../../base/lifecycle";
import { CONTEXT_KEY_SERVICE } from "../../services/context-key-service";
import { registerPanelFactory } from "../../services/panel-registry";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { registerCommand } from "../menu/command-registry";
import { STATUS_BAR } from "../status/status-bar";
import { focusWorkshopTree, toggleWorkshopPanel, WorkshopTreePanel } from "./workshop-panel";

/**
 * The layout directory's activation: installs the Workshop tree's panel
 * factory and the tree's commands (the Ctrl+B toggle and the Ctrl+Shift+F
 * focus, bound in shortcuts.ts), and binds the sidebar visibility context
 * keys the Appearance menu's checkboxes read - both bars are visible in
 * the boot layout, so the defaults are true. Called once by the panel
 * registry when the directory's chunk first loads; the returned
 * disposable is held for the page lifetime.
 */
export function register(): IDisposable {
  const contextKeys = getService(CONTEXT_KEY_SERVICE);
  contextKeys.createKey("sideBarVisible", true);
  contextKeys.createKey("auxiliaryBarVisible", true);
  const store = new DisposableStore();
  store.add(
    registerPanelFactory("tree", () => new WorkshopTreePanel(getServiceOrNull(STATUS_BAR))),
  );
  store.add(
    registerCommand("workshop.togglePanel", {
      label: "Workshop Panel",
      shortcut: "Ctrl+B",
      run: toggleWorkshopPanel,
    }),
  );
  store.add(registerCommand("workshop.focusTree", { run: focusWorkshopTree }));
  return store;
}
