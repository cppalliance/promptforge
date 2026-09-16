// The gateway contribution: the eager module registering the Settings
// row (plan step 19) at module scope, before any service exists.
// Settings is the workshop's one settings surface - the Gateway Config
// panel - so the row lives at File > Preferences > Settings with the
// ctrlcmd+, chord (Cmd+, on macOS), where Cursor keeps its settings
// rows. The run body opens the config panel through the zone registry;
// the panel's chunk loads lazily through the panel registry, and a
// second activation focuses the existing panel because the config panel
// is a singleton.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { MenuId } from "../../services/menu-registry";
import { openInZone } from "../layout/zones";

/** The Preferences flyout's id; the menubar contribution declares the submenu. */
const PREFERENCES_MENU: MenuId = "menubar/file/preferences";

const action: ActionDescriptor = {
  id: "workbench.action.openSettings",
  title: "Settings",
  f1: true,
  keybinding: { keybinding: "ctrlcmd+," },
  menu: [{ id: PREFERENCES_MENU, group: "1_settings" }],
  run: () => {
    openInZone("config", {});
  },
};
const result: Result<IDisposable, ParseError> = registerAction(action);
if (!result.ok) {
  console.error(`gateway action '${action.id}': ${result.error.message}`);
}
