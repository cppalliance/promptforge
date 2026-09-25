// The status contribution: the eager module registering the Status Bar
// visibility row at module scope, before any service exists. The run
// body resolves the composition root's StatusBar at call time and
// flips it; the bar itself mirrors the outcome into the
// statusBarVisible context key, which the Appearance row reads for its
// checkbox. With no composition root (a standalone widget test) there is
// no bar to flip and the row is inert.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { MenuId } from "../../services/menu-registry";
import { getServiceOrNull } from "../../services/service-registry";
import { STATUS_BAR } from "../../services/status-bar";

/** The Appearance flyout's id; the menubar contribution declares the submenu. */
const APPEARANCE_MENU: MenuId = "menubar/view/appearance";

const result: Result<IDisposable, ParseError> = registerAction({
  id: "workbench.action.toggleStatusbarVisibility",
  title: "Status Bar",
  f1: true,
  toggled: "statusBarVisible",
  menu: [{ id: APPEARANCE_MENU, group: "2_workbench_layout", order: 4 }],
  run: () => {
    const bar = getServiceOrNull(STATUS_BAR);
    if (bar !== null) {
      bar.setVisible(!bar.isVisible);
    }
  },
});
if (!result.ok) {
  console.error(`status action 'workbench.action.toggleStatusbarVisibility': ${result.error.message}`);
}
