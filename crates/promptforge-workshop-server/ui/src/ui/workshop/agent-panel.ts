// The agent-session surface as a Dockview panel: one socket, one
// session, one panel - the modal design. The panel composes the wire
// (AgentSocket), the state (AgentSessionService), and the session view.
// Closing the panel disposes the tree and closes its socket; every new
// panel gets a fresh socket and therefore a fresh server session.

import type { IContentRenderer } from "dockview";

import { Disposable } from "../../base/lifecycle";
import { AgentSessionService } from "../../services/agent-session";
import { AgentSocket } from "../../services/agent-socket";
import type { ModelService } from "../../services/model-service";
import { AgentSessionView } from "../agent-session-view";
import type { SttStatus } from "../stt";

// Where the session view's dictation reports when the panel is built without
// the composition root's status bar (the registry tests): messages and
// the recording LED have nowhere to land, so they land nowhere.
const SILENT_STATUS: SttStatus = {
  showLocal: () => undefined,
  setRecording: () => undefined,
};

export class AgentPanel extends Disposable implements IContentRenderer {
  readonly element = document.createElement("div");

  constructor(
    private readonly status: SttStatus = SILENT_STATUS,
    private readonly modelService?: ModelService,
  ) {
    super();
    this.element.className = "agent-panel";
  }

  init(): void {
    const socket = this._register(new AgentSocket());
    const service = this._register(new AgentSessionService(socket));
    const view = this._register(new AgentSessionView(service, this.status, this.modelService));
    this.element.appendChild(view.element);
    this._register(
      service.onDidChangeAgents((agents) => {
        if (agents.length > 0 && service.session === null) {
          const target = agents.includes("chat") ? "chat" : agents[0]!;
          service.launch(target);
        }
      }),
    );
    socket.connect();
  }
}
