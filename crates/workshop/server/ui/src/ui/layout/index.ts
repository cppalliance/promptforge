// The layout feature's entry point: register() is the panel registry's
// activation hook. Importers point at the source files directly; this
// module re-exports nothing.

import { DisposableStore, type IDisposable } from "../../base/lifecycle";
import { CONTEXT_KEY_SERVICE } from "../../services/context-key-service";
import { registerPanelFactory } from "../../services/panel-registry";
import { getService, getServiceOrNull } from "../../services/service-registry";
import { STATUS_BAR } from "../status/status-bar";
import { PlaceholderPanel } from "./placeholder-panel";
import { WorkshopTreePanel } from "./workshop-panel";

/**
 * The layout directory's activation: installs the Workshop tree's panel
 * factory and binds the sidebar visibility context keys the Appearance
 * menu's checkboxes read - both bars are visible in the boot layout, so
 * the defaults are true. The tree's actions and keybindings (the
 * Ctrl+B toggle, the Ctrl+Shift+E focus) register eagerly from
 * layout.contribution.ts. Called once by the panel registry when the
 * directory's chunk first loads; the returned disposable is held for
 * the page lifetime.
 */
export function register(): IDisposable {
  const contextKeys = getService(CONTEXT_KEY_SERVICE);
  contextKeys.createKey("sideBarVisible", true);
  contextKeys.createKey("auxiliaryBarVisible", true);
  const store = new DisposableStore();
  store.add(
    registerPanelFactory("tree", () => new WorkshopTreePanel(getServiceOrNull(STATUS_BAR))),
  );
  // The placeholder rides this chunk: it is the layout layer's own
  // panel, installed beside the tree's factory.
  store.add(registerPanelFactory("placeholder", () => new PlaceholderPanel()));
  return store;
}
