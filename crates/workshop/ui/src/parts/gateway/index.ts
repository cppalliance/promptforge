// The gateway feature's entry point: register() is the panel registry's
// activation hook. Importers point at the source files directly; this
// module re-exports nothing.

import type { IDisposable } from "../../base/lifecycle";
import { registerPanelFactory } from "../../services/panel-registry";
import { GatewayConfigPanel } from "./gateway-config-panel";

/**
 * The gateway directory's activation: installs the Gateway Config panel
 * factory. Called once by the panel registry when the directory's chunk
 * first loads; the returned disposable is held for the page lifetime.
 */
export function register(): IDisposable {
  return registerPanelFactory("config", () => new GatewayConfigPanel());
}
