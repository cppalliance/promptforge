// Dictation's status port: where a take reports what the user should see.
// The composition root builds it over the status bar and the recording
// indicator it owns, and registers it before the layout boots, so every
// agent panel - restored, seeded, or opened later - resolves the live port.

import { createServiceToken, type ServiceToken } from "@workshop/platform/service-registry";

/**
 * The slice dictation paints: local messages (blockers, capture failures,
 * an empty take) and the recording LED. Tests hand in a recording fake.
 */
export interface SttStatus {
  showLocal(label: string, severity: "info" | "error"): void;
  setRecording(on: boolean): void;
}

/**
 * The registry token for dictation's status port. Unregistered in tests
 * that drive panels standalone, where the panels stay silent.
 */
export const STT_STATUS: ServiceToken<SttStatus> = createServiceToken<SttStatus>("workshop.sttStatus");
