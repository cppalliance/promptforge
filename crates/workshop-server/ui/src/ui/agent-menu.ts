// The agent menu: the list of discovered agents an operator launches a
// session from. Renders from the delegate's agent list, re-renders on
// every push (the list is a complete snapshot per connect), and disables
// its buttons after a launch goes out - the session acknowledgment hides
// the whole menu, and an error frame (a refused launch) re-enables it
// with the server's message shown. The one locally-authored message is
// for the failure the server can never report: the socket is down and
// the launch never left.

import "./agent-session.css";

import type { Event } from "../base/event";
import { Disposable } from "../base/lifecycle";

/**
 * The slice of agent-session state the menu reads and dispatches
 * through; `AgentSessionService` satisfies it structurally.
 */
export interface AgentMenuDelegate {
  /** The discovered agent names, as last pushed by the server. */
  readonly agents: readonly string[];
  readonly onDidChangeAgents: Event<readonly string[]>;
  /** Fires with every error the session surface folded, message as shown. */
  readonly onError: Event<string>;
  /** Asks the server to launch the agent; false when the socket is down. */
  launch(agent: string): boolean;
}

/** The launchable-agent list, shown while the panel has no session. */
export class AgentMenu extends Disposable {
  readonly element: HTMLElement;
  private readonly list: HTMLUListElement;
  private readonly empty: HTMLParagraphElement;
  private readonly errorLine: HTMLParagraphElement;
  /** True from a sent launch until an error frame frees the menu. */
  private launching = false;

  constructor(private readonly delegate: AgentMenuDelegate) {
    super();
    this.element = document.createElement("section");
    this.element.className = "agent-menu";
    this.element.setAttribute("aria-label", "Agents");

    const lead = document.createElement("p");
    lead.className = "agent-menu__lead";
    lead.textContent = "Launch an agent to start a session.";

    this.list = document.createElement("ul");
    this.list.className = "agent-menu__list";

    this.empty = document.createElement("p");
    this.empty.className = "agent-menu__empty";
    this.empty.textContent = "No agents discovered.";

    this.errorLine = document.createElement("p");
    this.errorLine.className = "agent-menu__error";
    this.errorLine.hidden = true;

    this.element.append(lead, this.list, this.empty, this.errorLine);

    this._register(this.delegate.onDidChangeAgents(() => this.render()));
    this._register(
      this.delegate.onError((message) => {
        // A refused launch answers with an error frame; the menu frees
        // itself for another try and shows the server's message.
        this.launching = false;
        this.errorLine.textContent = message;
        this.errorLine.hidden = false;
        this.render();
      }),
    );
    this.render();
  }

  private render(): void {
    const agents = this.delegate.agents;
    this.list.replaceChildren();
    for (const agent of agents) {
      const entry = document.createElement("li");
      const launch = document.createElement("button");
      launch.type = "button";
      launch.className = "agent-menu__launch";
      launch.textContent = agent;
      launch.disabled = this.launching;
      launch.addEventListener("click", () => this.launch(agent));
      entry.appendChild(launch);
      this.list.appendChild(entry);
    }
    this.list.hidden = agents.length === 0;
    this.empty.hidden = agents.length > 0;
  }

  private launch(agent: string): void {
    if (this.launching) {
      return;
    }
    this.errorLine.hidden = true;
    if (!this.delegate.launch(agent)) {
      this.errorLine.textContent =
        "The agent socket is down; it reconnects by itself. Try again shortly.";
      this.errorLine.hidden = false;
      return;
    }
    this.launching = true;
    this.render();
  }
}
