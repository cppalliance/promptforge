// The agent-session surface as a Dockview panel: one socket, one
// session, one window - the modal design. The panel composes the wire
// (AgentSocket), the state (AgentSessionService), and the two views: the
// agent menu shows until a session is acknowledged, then the session
// view owns the panel for the panel's lifetime. Closing the panel
// disposes the tree, which closes the socket; the server-side session
// survives it by design, and a fresh panel starts from the menu again.

import type { IContentRenderer } from "dockview";

import { Disposable } from "../../base/lifecycle";
import { AgentSessionService } from "../../services/agent-session";
import { AgentSocket } from "../../services/agent-socket";
import type { ModelService } from "../../services/model-service";
import { AgentMenu } from "../agent-menu";
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
    const menu = this._register(new AgentMenu(service));
    const view = this._register(new AgentSessionView(service, this.status, this.modelService));
    view.element.hidden = true;
    this.element.append(menu.element, view.element);
    this._register(
      service.onDidChangeSession(() => {
        // The first acknowledgment swaps the menu away for good; agent
        // windows are modal, so no path leads back to the menu.
        menu.element.hidden = true;
        view.element.hidden = false;
      }),
    );
    // Construct-subscribe-connect: every handler above is wired before
    // the socket opens, so no push can precede it.
    socket.connect();
  }
}
