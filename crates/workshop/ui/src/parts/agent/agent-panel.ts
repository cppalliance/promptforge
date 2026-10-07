// The agent-session surface as a Dockview panel: one socket, one
// session, one panel - the modal design. The panel composes the wire
// (AgentSocket), the state (AgentSessionService), and the session view.
// Closing the panel disposes the tree and closes its socket; every new
// panel gets a fresh socket and therefore a fresh server session.

import type { GroupPanelPartInitParameters } from "dockview";
import { WorkshopPart } from "@workshop/platform/workshop-part";
import { AgentSessionService } from "../../services/agent-session";
import { AgentSocket } from "../../services/agent-socket";
import type { ModelService } from "../../services/model-service";
import type { SpeechCaptureService } from "../../services/speech-capture";
import type { SttStatus } from "../../services/stt-status";
import { AgentSessionView } from "./agent-session-view";

// Where the session view's dictation reports when the panel is built without
// the composition root's status port (the registry tests): messages and
// the recording LED have nowhere to land, so they land nowhere.
const SILENT_STATUS: SttStatus = {
  showLocal: () => undefined,
  setRecording: () => undefined,
};

export class AgentPanel extends WorkshopPart {
  private instance: string | undefined;

  constructor(
    private readonly status: SttStatus = SILENT_STATUS,
    private readonly modelService?: ModelService,
    private readonly speechCapture?: SpeechCaptureService,
  ) {
    super();
    this.element.className = "ws-agent-panel";
  }

  override init(parameters: GroupPanelPartInitParameters): void {
    const instance = parameters.params?.instance;
    this.instance = typeof instance === "string" ? instance : undefined;
    super.init(parameters);
  }

  protected create(parent: HTMLElement): void {
    const socket = this._register(new AgentSocket());
    const service = this._register(new AgentSessionService(socket));
    const view = this._register(
      new AgentSessionView(service, this.status, this.modelService, this.speechCapture, {
        instance: this.instance,
        title: () => this.panelApi?.title,
      }),
    );
    parent.appendChild(view.element);
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
