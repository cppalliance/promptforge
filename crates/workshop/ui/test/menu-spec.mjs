// The menu-spec test (plan step 20): the plan's Menu spec and Flyout
// contents restated as one table, driven against the shared registries
// after bundling every contribution module - the menubar and stub
// tables from this step plus the feature contributions from steps
// 13-19. Covers: the eight top-level menus in order; every spec row in
// its menu and group, in spec render order within each menu; every
// wired row listed in the command palette (f1); every stub row
// registered with precondition "false" (always disabled) and absent
// from the palette; the constant toggled expressions on the checkable
// stub rows (Menu Bar and Panel checked, the rest unchecked, one radio
// default per group).
//
// Two catalog rows are wired but deliberately not f1 and are excluded
// from the palette check: vscode.open (a palette row cannot supply its
// path argument) and workbench.action.quickOpenWithModes /
// quickOpenHelp (the command-center pill's rows, not menu-spec rows).
//
// Run: node --test test/menu-spec.mjs
import { writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as esbuild from "esbuild";
import { JSDOM } from "jsdom";

const uiDir = path.dirname(fileURLToPath(import.meta.url));

const bundle = await esbuild.build({
  stdin: {
    contents: `
      import "./src/parts/menu/menubar.contribution.ts";
      import "./src/parts/menu/stubs.contribution.ts";
      import "./src/parts/menu/edit.contribution.ts";
      import "./src/parts/editor/editor.contribution.ts";
      import "./src/parts/workspace/files.contribution.ts";
      import "./src/parts/workspace-files/workspace-files.contribution.ts";
      import "./src/parts/chrome/chrome.contribution.ts";
      import "./src/parts/layout/layout.contribution.ts";
      import "./src/parts/status/status.contribution.ts";
      import "./src/parts/agent/agent.contribution.ts";
      import "./src/parts/gateway/gateway.contribution.ts";
      import "./src/parts/run/run.contribution.ts";
      import "./src/parts/quickinput/quickinput.contribution.ts";
      export { Commands } from "./src/services/command-registry.ts";
      export { Menus, MenuId } from "./src/services/menu-registry.ts";
      export { registerService } from "./src/services/service-registry.ts";
      export { TREE_STATE } from "./src/services/tree-state-service.ts";
      export { RECENT_FILES_STORE } from "./src/services/recent-files-store.ts";
    `,
    resolveDir: path.join(uiDir, ".."),
    loader: "ts",
  },
  bundle: true,
  write: false,
  format: "esm",
  platform: "browser",
  target: "es2022",
  logLevel: "silent",
  // The modules under test import colocated CSS; the test drives only
  // the JS, and jsdom applies no stylesheets anyway.
  loader: { ".css": "empty" },
  alias: {
    "@tauri-apps/plugin-dialog": path.join(uiDir, "helpers", "tauri-dialog-stub.mjs"),
    "@tauri-apps/api/event": path.join(uiDir, "helpers", "tauri-event-stub.mjs"),
    "@tauri-apps/api/window": path.join(uiDir, "helpers", "tauri-window-stub.mjs"),
    "@tauri-apps/api/webviewWindow": path.join(uiDir, "helpers", "tauri-webview-stub.mjs"),
  },
});

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://127.0.0.1:7912/",
  pretendToBeVisual: true,
});
const { window } = dom;
globalThis.window = window;
globalThis.document = window.document;

// The contributions register at module scope; a malformed descriptor
// reports through console.error, so spy on it across the bundle import.
const consoleErrors = [];
const realConsoleError = console.error;
console.error = (...args) => {
  consoleErrors.push(args.join(" "));
};

const bundlePath = path.join(os.tmpdir(), "promptforge-step20-test.mjs");
await writeFile(bundlePath, bundle.outputFiles[0].text);
const { Commands, Menus, registerService, TREE_STATE, RECENT_FILES_STORE } = await import(
  pathToFileURL(bundlePath).href
);
console.error = realConsoleError;

const failures = [];
function check(name, condition) {
  if (!condition) failures.push(name);
}

check("no contribution reported a malformed descriptor", consoleErrors.length === 0);

// The Open Recent provider resolves its stores at call time; empty
// fakes keep the dynamic half row-free so the static rows stand alone.
registerService(TREE_STATE, () => ({ listing: () => undefined, cachedListings: () => [] }));
registerService(RECENT_FILES_STORE, () => ({ list: [], clear() {}, record() {} }));

// --- The spec table -----------------------------------------------------------
//
// One entry per menu, rows in spec render order. A row is
// [id, group, kind, title?, toggled?]: the command id or submenu id, the
// sort group (undefined for the groupless root menu), "wired" | "stub" |
// "sub", the row label for stub and submenu rows, and the constant
// toggled expression for checkable stub rows.
const SPEC = {
  menubar: [
    ["menubar/file", undefined, "sub", "File"],
    ["menubar/edit", undefined, "sub", "Edit"],
    ["menubar/selection", undefined, "sub", "Selection"],
    ["menubar/view", undefined, "sub", "View"],
    ["menubar/go", undefined, "sub", "Go"],
    ["menubar/run", undefined, "sub", "Run"],
    ["menubar/terminal", undefined, "sub", "Terminal"],
    ["menubar/help", undefined, "sub", "Help"],
  ],
  "menubar/file": [
    ["workbench.action.files.newUntitledFile", "1_new", "wired"],
    ["workbench.action.newWindow", "1_new", "stub", "New Window"],
    ["workbench.action.newAgentsWindow", "1_new", "wired"],
    ["menubar/file/newWindowWithProfile", "1_new", "sub", "New Window with Profile"],
    ["workbench.action.files.openFile", "2_open", "wired"],
    ["workbench.action.files.openFolder", "2_open", "wired"],
    ["workbench.action.openWorkspace", "2_open", "wired", "Open Workspace from File..."],
    ["menubar/file/recent", "2_open", "sub", "Open Recent"],
    ["workbench.action.addRootFolder", "3_workspace", "wired"],
    ["workbench.action.saveWorkspaceAs", "3_workspace", "wired", "Save Workspace As..."],
    ["workbench.action.duplicateWorkspace", "3_workspace", "wired", "Duplicate Workspace..."],
    ["workbench.action.files.save", "4_save", "wired"],
    ["workbench.action.files.saveAs", "4_save", "wired"],
    ["workbench.action.files.saveAll", "4_save", "wired"],
    ["menubar/file/share", "5_share", "sub", "Share"],
    ["workbench.action.toggleAutoSave", "5_share", "stub", "Auto Save", "false"],
    ["menubar/file/preferences", "5_share", "sub", "Preferences"],
    ["workbench.action.files.revert", "6_close", "wired"],
    ["workbench.action.closeActiveEditor", "6_close", "wired"],
    ["workbench.action.closeFolder", "6_close", "stub", "Close Folder"],
    ["workbench.action.closeWindow", "6_close", "wired"],
    ["workbench.action.quit", "z_Exit", "wired"],
  ],
  "menubar/file/newWindowWithProfile": [
    ["workbench.profiles.actions.createProfile", "2_new", "stub", "New Profile..."],
  ],
  "menubar/file/recent": [
    ["workbench.action.reopenClosedEditor", "1_editor", "wired"],
    ["workbench.action.openRecent", "y_more", "wired"],
    ["workbench.action.clearRecentFiles", "z_clear", "wired"],
  ],
  "menubar/file/share": [
    ["workbench.profiles.actions.exportProfile", "1_profiles", "stub", "Export Profile..."],
    ["workbench.profiles.actions.importProfile", "1_profiles", "stub", "Import Profile..."],
  ],
  "menubar/file/preferences": [
    ["workbench.profiles.actions.manageProfiles", "1_settings", "stub", "Profiles"],
    ["workbench.action.openSettings", "1_settings", "wired"],
    ["workbench.view.extensions", "1_settings", "stub", "Extensions"],
    ["workbench.action.openGlobalKeybindings", "1_settings", "stub", "Keyboard Shortcuts"],
    ["workbench.action.openSnippets", "1_settings", "stub", "Configure Snippets"],
    ["workbench.action.tasks.configureTaskRunner", "1_settings", "stub", "Tasks"],
    ["menubar/file/preferences/themes", "1_settings", "sub", "Themes"],
    ["workbench.action.openOnlineServicesSettings", "2_online", "stub", "Online Services Settings"],
  ],
  "menubar/file/preferences/themes": [
    ["workbench.action.selectTheme", "1_themes", "stub", "Color Theme"],
    ["workbench.action.selectIconTheme", "1_themes", "stub", "File Icon Theme"],
    ["workbench.action.selectProductIconTheme", "1_themes", "stub", "Product Icon Theme"],
  ],
  "menubar/edit": [
    ["undo", "1_do", "wired"],
    ["redo", "1_do", "wired"],
    ["editor.action.clipboardCutAction", "2_ccp", "wired"],
    ["editor.action.clipboardCopyAction", "2_ccp", "wired"],
    ["editor.action.clipboardPasteAction", "2_ccp", "wired"],
    ["actions.find", "3_find", "wired"],
    ["editor.action.startFindReplaceAction", "3_find", "wired"],
    ["workbench.action.findInFiles", "4_findInFiles", "stub", "Find in Files"],
    ["workbench.action.replaceInFiles", "4_findInFiles", "stub", "Replace in Files"],
    ["editor.action.commentLine", "5_insert", "wired"],
    ["editor.action.blockComment", "5_insert", "wired"],
    ["editor.emmet.action.expandAbbreviation", "5_insert", "stub", "Emmet: Expand Abbreviation"],
  ],
  "menubar/selection": [
    ["editor.action.selectAll", "1_basic", "wired"],
    ["editor.action.smartSelect.expand", "1_basic", "wired"],
    ["editor.action.smartSelect.shrink", "1_basic", "wired"],
    ["editor.action.copyLinesUpAction", "2_line", "wired"],
    ["editor.action.copyLinesDownAction", "2_line", "wired"],
    ["editor.action.moveLinesUpAction", "2_line", "wired"],
    ["editor.action.moveLinesDownAction", "2_line", "wired"],
    ["editor.action.duplicateSelection", "2_line", "wired"],
    ["editor.action.insertCursorAbove", "3_multi", "wired"],
    ["editor.action.insertCursorBelow", "3_multi", "wired"],
    ["editor.action.insertCursorAtEndOfEachLineSelected", "3_multi", "wired"],
    ["editor.action.addSelectionToNextFindMatch", "3_multi", "wired"],
    ["editor.action.addSelectionToPreviousFindMatch", "3_multi", "wired"],
    ["editor.action.selectHighlights", "3_multi", "wired"],
    ["editor.action.toggleMultiCursorModifier", "4_config", "stub", "Switch to Ctrl+Click for Multi-Cursor"],
    ["editor.action.toggleColumnSelection", "4_config", "wired"],
  ],
  "menubar/view": [
    ["workbench.action.showCommands", "1_open", "wired", "Command Palette..."],
    ["workbench.action.quickOpenView", "1_open", "stub", "Open View..."],
    ["menubar/view/appearance", "2_submenus", "sub", "Appearance"],
    ["menubar/view/editorLayout", "2_submenus", "sub", "Editor Layout"],
    ["workbench.view.explorer", "3_views", "wired"],
    ["workbench.view.search", "3_views", "stub", "Search"],
    ["workbench.view.scm", "3_views", "stub", "Source Control"],
    ["workbench.view.debug", "3_views", "stub", "Run"],
    ["workbench.view.extensions", "3_views", "stub", "Extensions"],
    ["workbench.actions.view.problems", "4_panels", "stub", "Problems"],
    ["workbench.action.output.toggleOutput", "4_panels", "stub", "Output"],
    ["workbench.debug.action.toggleRepl", "4_panels", "stub", "Debug Console"],
    ["workbench.action.terminal.toggleTerminal", "4_panels", "stub", "Terminal"],
    ["editor.action.toggleWordWrap", "5_editor", "wired"],
  ],
  "menubar/view/appearance": [
    ["workbench.action.toggleFullScreen", "1_toggle_view", "wired"],
    ["workbench.action.toggleZenMode", "1_toggle_view", "stub", "Zen Mode"],
    ["workbench.action.toggleCenteredLayout", "1_toggle_view", "stub", "Centered Layout"],
    ["workbench.action.openBrowser", "1_toggle_view", "stub", "Open Browser"],
    ["workbench.action.toggleMenuBar", "2_workbench_layout", "stub", "Menu Bar", "true"],
    ["workbench.action.toggleSidebarVisibility", "2_workbench_layout", "wired"],
    ["workbench.action.toggleAuxiliaryBar", "2_workbench_layout", "wired"],
    ["workbench.action.toggleStatusbarVisibility", "2_workbench_layout", "wired"],
    ["workbench.action.togglePanel", "2_workbench_layout", "stub", "Panel", "true"],
    ["workbench.action.toggleSidebarPosition", "3_panel_layout", "stub", "Move Primary Side Bar Right"],
    ["menubar/view/appearance/panelPosition", "3_panel_layout", "sub", "Panel Position"],
    ["menubar/view/appearance/alignPanel", "3_panel_layout", "sub", "Align Panel"],
    ["menubar/view/appearance/tabBar", "3_panel_layout", "sub", "Tab Bar"],
    ["menubar/view/appearance/editorActionsPosition", "3_panel_layout", "sub", "Editor Actions Position"],
    ["editor.action.toggleMinimap", "4_editor", "stub", "Minimap", "false"],
    ["breadcrumbs.toggle", "4_editor", "stub", "Toggle Breadcrumbs"],
    ["editor.action.toggleStickyScroll", "4_editor", "stub", "Sticky Scroll", "false"],
    ["editor.action.toggleRenderWhitespace", "4_editor", "wired"],
    ["editor.action.toggleRenderControlCharacter", "4_editor", "wired"],
    ["workbench.action.zoomIn", "5_zoom", "wired"],
    ["workbench.action.zoomOut", "5_zoom", "wired"],
    ["workbench.action.zoomReset", "5_zoom", "wired"],
  ],
  "menubar/view/appearance/panelPosition": [
    ["workbench.action.positionPanelTop", "1_position", "stub", "Top", "false"],
    ["workbench.action.positionPanelLeft", "1_position", "stub", "Left", "false"],
    ["workbench.action.positionPanelRight", "1_position", "stub", "Right", "false"],
    ["workbench.action.positionPanelBottom", "1_position", "stub", "Bottom", "true"],
  ],
  "menubar/view/appearance/alignPanel": [
    ["workbench.action.alignPanelCenter", "1_align", "stub", "Center", "true"],
    ["workbench.action.alignPanelJustify", "1_align", "stub", "Justify", "false"],
    ["workbench.action.alignPanelLeft", "1_align", "stub", "Left", "false"],
    ["workbench.action.alignPanelRight", "1_align", "stub", "Right", "false"],
  ],
  "menubar/view/appearance/tabBar": [
    ["workbench.action.showMultipleEditorTabs", "1_tabs", "stub", "Multiple Tabs", "true"],
    ["workbench.action.showSingleEditorTab", "1_tabs", "stub", "Single Tab", "false"],
    ["workbench.action.hideEditorTabs", "1_tabs", "stub", "Hidden", "false"],
  ],
  "menubar/view/appearance/editorActionsPosition": [
    ["workbench.action.editorActionsPositionTabBar", "1_position", "stub", "Tab Bar", "true"],
    ["workbench.action.editorActionsPositionTitleBar", "1_position", "stub", "Title Bar", "false"],
    ["workbench.action.editorActionsPositionHidden", "1_position", "stub", "Hidden", "false"],
  ],
  "menubar/view/editorLayout": [
    ["workbench.action.splitEditorUp", "1_split", "wired"],
    ["workbench.action.splitEditorDown", "1_split", "wired"],
    ["workbench.action.splitEditorLeft", "1_split", "wired"],
    ["workbench.action.splitEditorRight", "1_split", "wired"],
    ["workbench.action.moveEditorToNewWindow", "2_new_window", "stub", "Move Editor into New Window"],
    ["workbench.action.copyEditorToNewWindow", "2_new_window", "stub", "Copy Editor into New Window"],
    ["workbench.action.editorLayoutSingle", "3_layout", "stub", "Single"],
    ["workbench.action.editorLayoutTwoColumns", "3_layout", "stub", "Two Columns"],
    ["workbench.action.editorLayoutThreeColumns", "3_layout", "stub", "Three Columns"],
    ["workbench.action.editorLayoutTwoRows", "3_layout", "stub", "Two Rows"],
    ["workbench.action.editorLayoutThreeRows", "3_layout", "stub", "Three Rows"],
    ["workbench.action.editorLayoutTwoByTwoGrid", "3_layout", "stub", "Grid (2x2)"],
    ["workbench.action.editorLayoutTwoRowsRight", "3_layout", "stub", "Two Rows Right"],
    ["workbench.action.editorLayoutTwoColumnsBottom", "3_layout", "stub", "Two Columns Bottom"],
    ["workbench.action.toggleEditorGroupLayout", "4_flip", "stub", "Flip Layout"],
  ],
  "menubar/go": [
    ["workbench.action.navigateBack", "1_back", "stub", "Back"],
    ["workbench.action.navigateForward", "1_back", "stub", "Forward"],
    ["workbench.action.navigateToLastEditLocation", "1_back", "stub", "Last Edit Location"],
    ["menubar/go/switchEditor", "2_switch", "sub", "Switch Editor"],
    ["menubar/go/switchGroup", "2_switch", "sub", "Switch Group"],
    ["workbench.action.quickOpen", "3_global_nav", "wired"],
    ["workbench.action.showAllSymbols", "3_global_nav", "stub", "Go to Symbol in Workspace..."],
    ["workbench.action.gotoSymbol", "4_symbol_nav", "stub", "Go to Symbol in Editor..."],
    ["editor.action.revealDefinition", "4_symbol_nav", "stub", "Go to Definition"],
    ["editor.action.revealDeclaration", "4_symbol_nav", "stub", "Go to Declaration"],
    ["editor.action.goToTypeDefinition", "4_symbol_nav", "stub", "Go to Type Definition"],
    ["editor.action.goToImplementation", "4_symbol_nav", "stub", "Go to Implementations"],
    ["workbench.action.addSymbolToCurrentChat", "4_symbol_nav", "stub", "Add Symbol to Current Chat"],
    ["editor.action.goToReferences", "4_symbol_nav", "stub", "Go to References"],
    ["workbench.action.addSymbolToNewChat", "4_symbol_nav", "stub", "Add Symbol to New Chat"],
    ["workbench.action.gotoLine", "5_infile_nav", "wired"],
    ["editor.action.jumpToBracket", "5_infile_nav", "wired"],
    ["editor.action.marker.nextInFiles", "6_problem_nav", "wired"],
    ["editor.action.marker.prevInFiles", "6_problem_nav", "wired"],
    ["workbench.action.editor.nextChange", "7_change_nav", "stub", "Next Change"],
    ["workbench.action.editor.previousChange", "7_change_nav", "stub", "Previous Change"],
  ],
  "menubar/go/switchEditor": [
    ["workbench.action.nextEditor", "1_sideBySide", "wired"],
    ["workbench.action.previousEditor", "1_sideBySide", "wired"],
    ["workbench.action.openNextRecentlyUsedEditor", "2_used", "stub", "Next Used Editor"],
    ["workbench.action.openPreviousRecentlyUsedEditor", "2_used", "stub", "Previous Used Editor"],
    ["workbench.action.nextEditorInGroup", "3_group", "stub", "Next Editor in Group"],
    ["workbench.action.previousEditorInGroup", "3_group", "stub", "Previous Editor in Group"],
    ["workbench.action.openNextRecentlyUsedEditorInGroup", "4_used_group", "stub", "Next Used Editor in Group"],
    ["workbench.action.openPreviousRecentlyUsedEditorInGroup", "4_used_group", "stub", "Previous Used Editor in Group"],
  ],
  "menubar/go/switchGroup": [
    ["workbench.action.focusFirstEditorGroup", "1_by_index", "stub", "Group 1"],
    ["workbench.action.focusSecondEditorGroup", "1_by_index", "stub", "Group 2"],
    ["workbench.action.focusThirdEditorGroup", "1_by_index", "stub", "Group 3"],
    ["workbench.action.focusFourthEditorGroup", "1_by_index", "stub", "Group 4"],
    ["workbench.action.focusFifthEditorGroup", "1_by_index", "stub", "Group 5"],
    ["workbench.action.focusNextGroup", "2_nav", "stub", "Next Group"],
    ["workbench.action.focusPreviousGroup", "2_nav", "stub", "Previous Group"],
    ["workbench.action.focusLeftGroup", "3_directional", "stub", "Group Left"],
    ["workbench.action.focusRightGroup", "3_directional", "stub", "Group Right"],
    ["workbench.action.focusAboveGroup", "3_directional", "stub", "Group Above"],
    ["workbench.action.focusBelowGroup", "3_directional", "stub", "Group Below"],
  ],
  "menubar/run": [
    ["workbench.action.newRunWindow", "0_run", "wired", "New Run Window"],
    ["workbench.action.debug.start", "1_debug", "stub", "Start Debugging"],
    ["workbench.action.debug.run", "1_debug", "stub", "Run Without Debugging"],
    ["workbench.action.debug.stop", "1_debug", "stub", "Stop Debugging"],
    ["workbench.action.debug.restart", "1_debug", "stub", "Restart Debugging"],
    ["workbench.action.debug.configure", "2_configurations", "stub", "Open Configurations"],
    ["workbench.action.debug.addConfiguration", "2_configurations", "stub", "Add Configuration..."],
    ["workbench.action.debug.stepOver", "3_step", "stub", "Step Over"],
    ["workbench.action.debug.stepInto", "3_step", "stub", "Step Into"],
    ["workbench.action.debug.stepOut", "3_step", "stub", "Step Out"],
    ["workbench.action.debug.continue", "3_step", "stub", "Continue"],
    ["editor.debug.action.toggleBreakpoint", "4_breakpoints", "stub", "Toggle Breakpoint"],
    ["menubar/run/newBreakpoint", "4_breakpoints", "sub", "New Breakpoint"],
    ["editor.debug.action.enableAllBreakpoints", "5_breakpoints", "stub", "Enable All Breakpoints"],
    ["editor.debug.action.disableAllBreakpoints", "5_breakpoints", "stub", "Disable All Breakpoints"],
    ["editor.debug.action.removeAllBreakpoints", "5_breakpoints", "stub", "Remove All Breakpoints"],
    ["workbench.action.debug.installAdditionalDebuggers", "6_install", "stub", "Install Additional Debuggers..."],
  ],
  "menubar/run/newBreakpoint": [
    ["editor.debug.action.conditionalBreakpoint", "1_breakpoints", "stub", "Conditional Breakpoint..."],
    ["editor.debug.action.editBreakpoint", "1_breakpoints", "stub", "Edit Breakpoint"],
    ["editor.debug.action.toggleInlineBreakpoint", "1_breakpoints", "stub", "Inline Breakpoint"],
    ["editor.debug.action.addFunctionBreakpoint", "1_breakpoints", "stub", "Function Breakpoint..."],
    ["editor.debug.action.addLogpoint", "1_breakpoints", "stub", "Logpoint..."],
    ["editor.debug.action.addTriggeredBreakpoint", "1_breakpoints", "stub", "Triggered Breakpoint..."],
  ],
  "menubar/terminal": [
    ["workbench.action.terminal.new", "1_terminal", "stub", "New Terminal"],
    ["workbench.action.terminal.split", "1_terminal", "stub", "Split Terminal"],
    ["workbench.action.tasks.runTask", "2_tasks", "stub", "Run Task..."],
    ["workbench.action.tasks.build", "2_tasks", "stub", "Run Build Task..."],
    ["workbench.action.terminal.runActiveFile", "2_tasks", "stub", "Run Active File"],
    ["workbench.action.terminal.runSelectedText", "2_tasks", "stub", "Run Selected Text"],
    ["workbench.action.tasks.showTasks", "3_running", "stub", "Show Running Tasks..."],
    ["workbench.action.tasks.restartTask", "3_running", "stub", "Restart Running Task..."],
    ["workbench.action.tasks.terminateTask", "3_running", "stub", "Terminate Task..."],
    ["workbench.action.tasks.configureTaskRunner", "4_configure", "stub", "Configure Tasks..."],
    ["workbench.action.tasks.configureDefaultBuildTask", "4_configure", "stub", "Configure Default Build Task..."],
  ],
  "menubar/help": [
    ["workbench.action.showCommands", "1_welcome", "wired", "Show All Commands"],
    ["update.showCurrentReleaseNotes", "2_notes", "stub", "Show Release Notes"],
    ["workbench.action.openIssueReporter", "3_feedback", "stub", "Report Issue"],
    ["workbench.action.giveFeedback", "3_feedback", "stub", "Give Feedback..."],
    ["workbench.action.openLicenseUrl", "4_license", "stub", "View License"],
    ["workbench.action.toggleDevTools", "5_devtools", "stub", "Toggle Developer Tools"],
    ["workbench.action.openProcessExplorer", "5_devtools", "stub", "Open Process Explorer"],
    ["workbench.action.showAboutDialog", "z_about", "wired"],
  ],
};

// --- Every spec row in its menu and group, in spec order ------------------------

for (const [menu, rows] of Object.entries(SPEC)) {
  const actual = Menus.getMenuItems(menu);
  const actualIds = actual.map((row) => row.command ?? row.submenu);
  const expectedIds = rows.map((row) => row[0]);
  check(
    `${menu}: exactly the spec's rows in spec order (got: ${actualIds.join(", ") || "none"})`,
    actualIds.join("|") === expectedIds.join("|"),
  );
  for (const [id, group, , title] of rows) {
    const row = actual.find((candidate) => (candidate.command ?? candidate.submenu) === id);
    check(`${menu}: ${id} sits in group ${group}`, row !== undefined && row.group === group);
    if (title !== undefined) {
      const shown = row?.title ?? Commands.lookup(id)?.title;
      check(`${menu}: ${id} is labeled '${title}' (got: '${shown}')`, shown === title);
    }
  }
}

// --- Every wired row f1, every stub disabled ------------------------------------

const palette = new Set(Menus.getMenuItems("commandPalette").map((row) => row.command));
const wiredIds = new Set();
const stubIds = new Set();
for (const rows of Object.values(SPEC)) {
  for (const [id, , kind] of rows) {
    if (kind === "wired") wiredIds.add(id);
    if (kind === "stub") stubIds.add(id);
  }
}

// Wired rows that deliberately omit f1: menu-only commands. New Run
// Window is one menu row by design (no palette row, no keybinding).
const WIRED_WITHOUT_F1 = new Set(["workbench.action.newRunWindow"]);

for (const id of wiredIds) {
  if (WIRED_WITHOUT_F1.has(id)) {
    check(`menu-only row '${id}' stays out of the palette`, !palette.has(id));
    continue;
  }
  check(`wired row '${id}' reaches the palette (f1)`, palette.has(id));
}
for (const id of stubIds) {
  const command = Commands.lookup(id);
  check(`stub '${id}' is registered`, command !== undefined);
  check(`stub '${id}' is always disabled`, command?.precondition === "false");
  check(`stub '${id}' stays out of the palette`, !palette.has(id));
}

// --- The constant toggled expressions on checkable stub rows ---------------------

for (const rows of Object.values(SPEC)) {
  for (const [id, , kind, , toggled] of rows) {
    if (kind === "stub" && toggled !== undefined) {
      check(`stub '${id}' declares toggled '${toggled}'`, Commands.lookup(id)?.toggled === toggled);
    }
  }
}

if (failures.length > 0) {
  console.error(`menu-spec: ${failures.length} failure(s)`);
  for (const failure of failures) console.error(`  - ${failure}`);
  process.exit(1);
}
console.log("menu-spec: all assertions passed");
