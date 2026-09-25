// The agent feature's entry point: register() is the panel registry's
// activation hook. Importers point at the source files directly; this
// module re-exports nothing.

import { registerPanelFactory } from "../../services/panel-registry";
import { MODEL_SERVICE } from "../../services/model-service";
import { getServiceOrNull } from "../../services/service-registry";
import { SPEECH_CAPTURE } from "../../services/speech-capture";
import type { IDisposable } from "../../base/lifecycle";
import { STATUS_BAR } from "../../services/status-bar";
import { AgentPanel } from "./agent-panel";
import { markdownReady } from "./markdown-render";

/**
 * The agent directory's activation: installs the agent-session panel
 * factory, resolving the composition root's services through the service
 * registry (absent in standalone panel tests, where the panel stays
 * silent). Called once by the panel registry when the directory's chunk
 * first loads; the returned disposable is held for the page lifetime.
 *
 * Code blocks in the agent feed highlight through Shiki, whose init is
 * async and started at module scope; register() awaits readiness (with
 * failures logged, not thrown) before answering so the panel registry
 * mounts the panel only after highlighting is settled - the first
 * painted message is never the degraded unhighlighted render. A failed
 * init must not break the panel - the renderer degrades to plain
 * <pre><code> blocks instead.
 */
export async function register(): Promise<IDisposable> {
  await markdownReady.catch((error: unknown) => {
    console.error(
      "markdown highlighting failed to initialize; code blocks render unhighlighted:",
      error,
    );
  });
  return registerPanelFactory(
    "agent",
    () =>
      new AgentPanel(
        getServiceOrNull(STATUS_BAR) ?? undefined,
        getServiceOrNull(MODEL_SERVICE) ?? undefined,
        getServiceOrNull(SPEECH_CAPTURE) ?? undefined,
      ),
  );
}
