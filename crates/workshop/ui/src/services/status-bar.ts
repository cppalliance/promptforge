// The status bar service contract: the shape panels resolve through the
// registry to paint action outcomes onto the bar. The implementation
// (parts/status/status-bar.ts) is DOM-bound - it builds the shared status
// bar view and appends it to the body - so only this interface and the
// token live here, in the DOM-free services layer.

import { createServiceToken, type ServiceToken } from "./service-registry";

/** The status-bar surface consumers resolve from the registry. */
export interface StatusBar {
  /** Shows a locally-originated message; the next observer frame overwrites it. */
  showLocal(label: string, severity: "info" | "error"): void;
  /** Whether the bar is currently shown. */
  readonly isVisible: boolean;
  /** Shows or hides the bar. */
  setVisible(visible: boolean): void;
  /** Lights or dims the recording LED with the mic's recording state. */
  setRecording(on: boolean): void;
}

/**
 * The registry token for the composition root's StatusBar, resolved by
 * panels that paint action outcomes onto it (the Workshop tree's grant
 * flows, the agent session's dictation reports). Registered by the
 * composition root at boot; unregistered in tests that drive panels
 * standalone, where the panels stay silent.
 */
export const STATUS_BAR: ServiceToken<StatusBar> =
  createServiceToken<StatusBar>("workshop.statusBar");
