// The toast stack's service token. The composition root builds the one
// shared stack (main.ts) and registers it here, so a view that raises a
// toast - the agent transcript's "Message copied to clipboard", the
// status bar, a failed command - resolves it through the service registry
// instead of importing the app. Unregistered in tests that drive a panel
// standalone, where the caller stays silent.

import type { ToastStack } from "@workshop/look/toast";
import { createServiceToken, type ServiceToken } from "@workshop/platform/service-registry";

/** The registry token for the shared toast stack. */
export const TOAST_STACK: ServiceToken<ToastStack> =
  createServiceToken<ToastStack>("workshop.toastStack");
