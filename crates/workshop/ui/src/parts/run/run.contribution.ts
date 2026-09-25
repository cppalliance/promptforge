// The run contribution: the eager module registering the New Run Window
// row at module scope, before any service exists. The run body opens a
// fresh Run panel keyed by a random instance id - each invocation is its
// own window in the main zone - seeded with the focused editor's file
// when there is one. main.ts imports zones.ts eagerly (it boots the
// dock through it), so the module is already loaded and the open call
// is direct; the editor command layer pulls CodeMirror, so it is
// lazy-imported here, and the run chunk itself still loads lazily
// through the panel registry.
//
// The id is ours: no VS Code or Cursor row backs this window. One menu
// item, no keybinding - F5 belongs to the full plan's Run Prompt
// command. The disabled debug stubs in stubs.contribution.ts stay
// untouched; this row lands in its own group above them.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction, type ActionDescriptor } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { MenuId } from "../../services/menu-registry";
import { DOCK } from "../../services/panel-registry";
import { getService } from "../../services/service-registry";
import { openInZone } from "../layout/zones";

const action: ActionDescriptor = {
  id: "workbench.action.newRunWindow",
  title: "New Run Window",
  menu: [{ id: MenuId.MenubarRunMenu, group: "0_run", order: 1 }],
  run: () => {
    void import("../editor/editor-commands").then(({ asEditor }) => {
      openInZone("run", {
        instance: window.crypto.randomUUID(),
        path: asEditor(getService(DOCK).activePanel)?.filePath() ?? undefined,
      });
    });
  },
};
const result: Result<IDisposable, ParseError> = registerAction(action);
if (!result.ok) {
  console.error(`run action '${action.id}': ${result.error.message}`);
}
