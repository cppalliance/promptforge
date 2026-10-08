// The chat pane's header: three round-cornered buttons at the right end of
// the right zone's tab strip - New Agent (+), More Actions (...), and
// Close (x). Dockview builds one header renderer per group, so every group
// gets the element and the stylesheet shows it for the right zone's groups
// only (the zone stamp on the group, zones.ts). The buttons run commands
// through the command registry, so the keybindings, the palette, and the
// header share one behavior:
// - + runs New Chat Tab, which reuses an empty chat.
// - ... opens the pane menu (Toggle Chat Pane, then the three close rows)
//   at the button, with the group's active chat as the rows' target.
// - x runs Toggle Chat Pane, hiding the pane the header belongs to.
//
// The renderer owns no lifetime tree of its own: Dockview builds and
// disposes these through its own bookkeeping (one per refresh of a group),
// so the element and listeners are plain and dispose() drops the menu the
// buttons may have opened.

import "./agent-pane-header.css";

import type { DockviewGroupPanel, IGroupHeaderProps, IHeaderActionsRenderer } from "dockview";

import { ICON_ADD, ICON_CLOSE, ICON_ELLIPSIS } from "@workshop/look/icons";
import { Commands } from "@workshop/platform/command-registry";
import { Menu, reportCommandFailure } from "../menu/menu";
import { AGENT_PANE_MENU } from "./agent-commands";

/** One header button: a codicon on a 24px square, named for assistive tech and the tooltip. */
function headerButton(label: string, icon: string, onPress: (button: HTMLButtonElement) => void): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "ws-pane-header__button";
  button.setAttribute("aria-label", label);
  button.title = label;
  // A static string from @workshop/look, never data.
  button.innerHTML = icon;
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    onPress(button);
  });
  return button;
}

/** Runs a command from a header button, reporting a rejection on the status bar. */
function run(commandId: string): void {
  void Commands.execute(commandId).catch((error: unknown) => {
    reportCommandFailure(commandId, error);
  });
}

/** The right-group header actions: dockview's `createRightHeaderActionComponent`. */
class AgentPaneHeader implements IHeaderActionsRenderer {
  readonly element = document.createElement("div");
  private menu: Menu | null = null;

  constructor(private readonly group: DockviewGroupPanel) {
    this.element.className = "ws-pane-header";
    this.element.append(
      headerButton("New Agent", ICON_ADD, () => run("workbench.action.chat.newTab")),
      headerButton("More Actions...", ICON_ELLIPSIS, (button) => this.openMenu(button)),
      headerButton("Close", ICON_CLOSE, () => run("workbench.action.toggleAuxiliaryBar")),
    );
  }

  init(_params: IGroupHeaderProps): void {
    // Nothing to read from the group: the buttons act on commands.
  }

  dispose(): void {
    this.menu?.dispose();
    this.menu = null;
    this.element.remove();
  }

  private openMenu(anchor: HTMLElement): void {
    this.menu ??= new Menu();
    const active = this.group.activePanel;
    // The rows target this group's active tab, not whichever panel the
    // dock has active, so the menu works from a header that was never focused.
    this.menu.open(AGENT_PANE_MENU, anchor, active === undefined ? undefined : { panelId: active.id });
  }
}

/** Builds the header actions for one group. */
export function createAgentPaneHeader(group: DockviewGroupPanel): IHeaderActionsRenderer {
  return new AgentPaneHeader(group);
}
