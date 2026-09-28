// The agent contribution: the eager module registering the New Agents
// Window row at module scope, before any service exists.
// The run body opens a fresh agent panel keyed by a random instance id -
// each invocation is its own panel, socket, and modal server session in
// the right zone. main.ts imports zones.ts eagerly (it boots the dock
// through it), so the module is already loaded and the open call is
// direct; the agent chunk itself still loads lazily through the panel
// registry.
//
// The id is ours: Cursor ships a New Agents Window row but its command
// id is not public.
//
// The agent panel's type registers here too. Its instances key by the
// `instance` param; an open without one is the boot-time singleton.

import type { IDisposable } from "@workshop/platform/lifecycle";
import { registerAction, type ActionDescriptor } from "@workshop/platform/action-registry";
import type { ParseError } from "@workshop/platform/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { MenuId } from "@workshop/platform/menu-registry";
import { registerPanelType } from "@workshop/platform/panel-registry";
import { openInZone } from "../layout/zones";

registerPanelType({
  type: "agent",
  title: "Agent Session",
  defaultZone: "right",
  load: () => import("./index"),
});

const action: ActionDescriptor = {
  id: "workbench.action.newAgentsWindow",
  title: "New Agents Window",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+alt+n" },
  menu: [{ id: MenuId.MenubarFileMenu, group: "1_new", order: 3 }],
  run: () => {
    openInZone("agent", { instance: window.crypto.randomUUID() });
  },
};
const result: Result<IDisposable, ParseError> = registerAction(action);
if (!result.ok) {
  console.error(`agent action '${action.id}': ${result.error.message}`);
}
