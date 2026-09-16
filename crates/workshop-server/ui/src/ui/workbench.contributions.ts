// The workbench's contribution surface: one flat list of side-effect
// imports, one per feature contribution module. Each module registers
// its actions, menu rows, keybinding rules, and quick-access providers
// into the shared registries at module scope, before any service
// exists; run bodies resolve services at call time and lazy-import
// anything that pulls CodeMirror or dockview, so this list stays in the
// entry bundle without dragging the feature chunks with it. The menu
// feature's bootstrap (ui/menu/index.ts) imports this module once;
// no other app module imports the contribution files directly; tests
// bundle them to assert their registrations.
//
// Order is registration order, not sort order: the menu registry sorts
// rows by group, order, and title at read time, and the first
// keybinding rule registered for a command owns its menu label.

import "./menu/menubar.contribution";
import "./menu/stubs.contribution";
import "./menu/edit.contribution";
import "./editor/editor.contribution";
import "./workspace/files.contribution";
import "./chrome/chrome.contribution";
import "./layout/layout.contribution";
import "./status/status.contribution";
import "./agent/agent.contribution";
import "./gateway/gateway.contribution";
import "./quickinput/quickinput.contribution";
