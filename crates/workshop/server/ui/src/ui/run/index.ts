// The run feature's entry point: register() is the panel registry's
// activation hook. Importers point at the source files directly; this
// module re-exports nothing.

import type { IDisposable } from "../../base/lifecycle";
import { registerPanelFactory } from "../../services/panel-registry";
import { RunPanel } from "./run-panel";

/**
 * The run directory's activation: installs the Run window's panel
 * factory. The window's menu action registers eagerly from
 * run.contribution.ts. Called once by the panel registry when the
 * directory's chunk first loads; the returned disposable is held for
 * the page lifetime.
 */
export function register(): IDisposable {
  return registerPanelFactory("run", () => new RunPanel());
}
