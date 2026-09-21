// The stub table (plan step 20): every menu row Cursor shows that the
// workshop cannot back yet, registered eagerly at module scope under its
// VS Code command id with precondition "false" and a no-op run, so the
// row renders disabled with its final name and shortcut and implementing
// it later means deleting the row here and adding a registerAction in
// the owning feature - no menu, test, or keybinding changes. The
// precondition is also ANDed into the keybinding rule's when, so a
// stub's chord never dispatches; it exists so the disabled row renders
// its shortcut label.
//
// Rows Cursor shows checked that cannot be backed (Menu Bar, Panel)
// declare a constant-true toggled; the other checkable stubs (Auto Save,
// Minimap, Sticky Scroll, and the radio flyouts) declare constant
// expressions with one default checked per radio group. Cursor-only
// rows with no public id (Open Browser, the Add Symbol rows, Give
// Feedback, Online Services Settings) use ids under the
// workbench.action.* namespace.
//
// Emmet: Expand Abbreviation is the one row whose spec chord (Tab) is
// not registered: the dispatcher swallows any claimed chord even when
// its when fails, so a Tab rule would eat Tab in every editor. The row
// renders without a shortcut label until it is implemented.

import type { IDisposable } from "../../base/lifecycle";
import { registerAction } from "../../services/action-registry";
import type { ParseError } from "../../services/context-key-expr";
import type { Result } from "../../services/error-catalog";
import { appendMenuItem, MenuId } from "../../services/menu-registry";

/** One stub row: a disabled command with its menu placement and shortcut label. */
interface StubRow {
  /** The command id, VS Code's where one is public. */
  readonly id: string;
  /** The row label. */
  readonly title: string;
  /** The menu the row lands in. */
  readonly menu: MenuId;
  /** The sort group, so separators fall where the spec has them. */
  readonly group: string;
  /** The position within the group, matching the spec's row order. */
  readonly order: number;
  /** The spec's chord, bound so the disabled row renders its label. */
  readonly keybinding?: string;
  /** The constant toggled expression on checkable rows. */
  readonly toggled?: string;
}

const stubRows = [
  // File
  { id: "workbench.action.newWindow", title: "New Window", menu: MenuId.MenubarFileMenu, group: "1_new", order: 2, keybinding: "ctrlcmd+shift+n" },
  // Open Workspace from File..., Save Workspace As..., and Duplicate Workspace... are wired by the workspace-files contribution.
  { id: "workbench.action.toggleAutoSave", title: "Auto Save", menu: MenuId.MenubarFileMenu, group: "5_share", order: 2, toggled: "false" },
  { id: "workbench.action.closeFolder", title: "Close Folder", menu: MenuId.MenubarFileMenu, group: "6_close", order: 3, keybinding: "ctrlcmd+m f" },
  // File > New Window with Profile (the dynamic profile rows have no backing store)
  { id: "workbench.profiles.actions.createProfile", title: "New Profile...", menu: "menubar/file/newWindowWithProfile", group: "2_new", order: 1 },
  // File > Share
  { id: "workbench.profiles.actions.exportProfile", title: "Export Profile...", menu: "menubar/file/share", group: "1_profiles", order: 1 },
  { id: "workbench.profiles.actions.importProfile", title: "Import Profile...", menu: "menubar/file/share", group: "1_profiles", order: 2 },
  // File > Preferences (Settings is wired by the gateway contribution; Extensions and Tasks land here as second placements below)
  { id: "workbench.profiles.actions.manageProfiles", title: "Profiles", menu: "menubar/file/preferences", group: "1_settings", order: 1 },
  { id: "workbench.action.openGlobalKeybindings", title: "Keyboard Shortcuts", menu: "menubar/file/preferences", group: "1_settings", order: 4, keybinding: "ctrlcmd+m ctrlcmd+s" },
  { id: "workbench.action.openSnippets", title: "Configure Snippets", menu: "menubar/file/preferences", group: "1_settings", order: 5 },
  { id: "workbench.action.openOnlineServicesSettings", title: "Online Services Settings", menu: "menubar/file/preferences", group: "2_online", order: 1 },
  // File > Preferences > Themes
  { id: "workbench.action.selectTheme", title: "Color Theme", menu: "menubar/file/preferences/themes", group: "1_themes", order: 1, keybinding: "ctrlcmd+m ctrlcmd+t" },
  { id: "workbench.action.selectIconTheme", title: "File Icon Theme", menu: "menubar/file/preferences/themes", group: "1_themes", order: 2 },
  { id: "workbench.action.selectProductIconTheme", title: "Product Icon Theme", menu: "menubar/file/preferences/themes", group: "1_themes", order: 3 },
  // Edit
  { id: "workbench.action.findInFiles", title: "Find in Files", menu: MenuId.MenubarEditMenu, group: "4_findInFiles", order: 1, keybinding: "ctrlcmd+shift+f" },
  { id: "workbench.action.replaceInFiles", title: "Replace in Files", menu: MenuId.MenubarEditMenu, group: "4_findInFiles", order: 2, keybinding: "ctrlcmd+shift+h" },
  { id: "editor.emmet.action.expandAbbreviation", title: "Emmet: Expand Abbreviation", menu: MenuId.MenubarEditMenu, group: "5_insert", order: 3 },
  // Selection
  { id: "editor.action.toggleMultiCursorModifier", title: "Switch to Ctrl+Click for Multi-Cursor", menu: MenuId.MenubarSelectionMenu, group: "4_config", order: 1 },
  // View
  { id: "workbench.action.quickOpenView", title: "Open View...", menu: MenuId.MenubarViewMenu, group: "1_open", order: 2 },
  { id: "workbench.view.search", title: "Search", menu: MenuId.MenubarViewMenu, group: "3_views", order: 2, keybinding: "ctrlcmd+shift+f" },
  { id: "workbench.view.scm", title: "Source Control", menu: MenuId.MenubarViewMenu, group: "3_views", order: 3, keybinding: "ctrlcmd+shift+g" },
  { id: "workbench.view.debug", title: "Run", menu: MenuId.MenubarViewMenu, group: "3_views", order: 4, keybinding: "ctrlcmd+shift+d" },
  { id: "workbench.view.extensions", title: "Extensions", menu: MenuId.MenubarViewMenu, group: "3_views", order: 5, keybinding: "ctrlcmd+shift+x" },
  { id: "workbench.actions.view.problems", title: "Problems", menu: MenuId.MenubarViewMenu, group: "4_panels", order: 1, keybinding: "ctrlcmd+shift+m" },
  { id: "workbench.action.output.toggleOutput", title: "Output", menu: MenuId.MenubarViewMenu, group: "4_panels", order: 2, keybinding: "ctrlcmd+shift+u" },
  { id: "workbench.debug.action.toggleRepl", title: "Debug Console", menu: MenuId.MenubarViewMenu, group: "4_panels", order: 3, keybinding: "ctrlcmd+shift+alt+y" },
  { id: "workbench.action.terminal.toggleTerminal", title: "Terminal", menu: MenuId.MenubarViewMenu, group: "4_panels", order: 4, keybinding: "ctrlcmd+`" },
  // View > Appearance
  { id: "workbench.action.toggleZenMode", title: "Zen Mode", menu: "menubar/view/appearance", group: "1_toggle_view", order: 2, keybinding: "ctrlcmd+m z" },
  { id: "workbench.action.toggleCenteredLayout", title: "Centered Layout", menu: "menubar/view/appearance", group: "1_toggle_view", order: 3 },
  { id: "workbench.action.openBrowser", title: "Open Browser", menu: "menubar/view/appearance", group: "1_toggle_view", order: 4 },
  { id: "workbench.action.toggleMenuBar", title: "Menu Bar", menu: "menubar/view/appearance", group: "2_workbench_layout", order: 1, toggled: "true" },
  { id: "workbench.action.togglePanel", title: "Panel", menu: "menubar/view/appearance", group: "2_workbench_layout", order: 5, keybinding: "ctrlcmd+j", toggled: "true" },
  { id: "workbench.action.toggleSidebarPosition", title: "Move Primary Side Bar Right", menu: "menubar/view/appearance", group: "3_panel_layout", order: 1 },
  { id: "editor.action.toggleMinimap", title: "Minimap", menu: "menubar/view/appearance", group: "4_editor", order: 1, toggled: "false" },
  { id: "breadcrumbs.toggle", title: "Toggle Breadcrumbs", menu: "menubar/view/appearance", group: "4_editor", order: 2 },
  { id: "editor.action.toggleStickyScroll", title: "Sticky Scroll", menu: "menubar/view/appearance", group: "4_editor", order: 3, toggled: "false" },
  // View > Appearance > Panel Position (radio; Bottom is the default)
  { id: "workbench.action.positionPanelTop", title: "Top", menu: "menubar/view/appearance/panelPosition", group: "1_position", order: 1, toggled: "false" },
  { id: "workbench.action.positionPanelLeft", title: "Left", menu: "menubar/view/appearance/panelPosition", group: "1_position", order: 2, toggled: "false" },
  { id: "workbench.action.positionPanelRight", title: "Right", menu: "menubar/view/appearance/panelPosition", group: "1_position", order: 3, toggled: "false" },
  { id: "workbench.action.positionPanelBottom", title: "Bottom", menu: "menubar/view/appearance/panelPosition", group: "1_position", order: 4, toggled: "true" },
  // View > Appearance > Align Panel (radio; Center is the default)
  { id: "workbench.action.alignPanelCenter", title: "Center", menu: "menubar/view/appearance/alignPanel", group: "1_align", order: 1, toggled: "true" },
  { id: "workbench.action.alignPanelJustify", title: "Justify", menu: "menubar/view/appearance/alignPanel", group: "1_align", order: 2, toggled: "false" },
  { id: "workbench.action.alignPanelLeft", title: "Left", menu: "menubar/view/appearance/alignPanel", group: "1_align", order: 3, toggled: "false" },
  { id: "workbench.action.alignPanelRight", title: "Right", menu: "menubar/view/appearance/alignPanel", group: "1_align", order: 4, toggled: "false" },
  // View > Appearance > Tab Bar (radio; Multiple Tabs is the default)
  { id: "workbench.action.showMultipleEditorTabs", title: "Multiple Tabs", menu: "menubar/view/appearance/tabBar", group: "1_tabs", order: 1, toggled: "true" },
  { id: "workbench.action.showSingleEditorTab", title: "Single Tab", menu: "menubar/view/appearance/tabBar", group: "1_tabs", order: 2, toggled: "false" },
  { id: "workbench.action.hideEditorTabs", title: "Hidden", menu: "menubar/view/appearance/tabBar", group: "1_tabs", order: 3, toggled: "false" },
  // View > Appearance > Editor Actions Position (radio; Tab Bar is the default)
  { id: "workbench.action.editorActionsPositionTabBar", title: "Tab Bar", menu: "menubar/view/appearance/editorActionsPosition", group: "1_position", order: 1, toggled: "true" },
  { id: "workbench.action.editorActionsPositionTitleBar", title: "Title Bar", menu: "menubar/view/appearance/editorActionsPosition", group: "1_position", order: 2, toggled: "false" },
  { id: "workbench.action.editorActionsPositionHidden", title: "Hidden", menu: "menubar/view/appearance/editorActionsPosition", group: "1_position", order: 3, toggled: "false" },
  // View > Editor Layout
  { id: "workbench.action.moveEditorToNewWindow", title: "Move Editor into New Window", menu: "menubar/view/editorLayout", group: "2_new_window", order: 1 },
  { id: "workbench.action.copyEditorToNewWindow", title: "Copy Editor into New Window", menu: "menubar/view/editorLayout", group: "2_new_window", order: 2, keybinding: "ctrlcmd+m o" },
  { id: "workbench.action.editorLayoutSingle", title: "Single", menu: "menubar/view/editorLayout", group: "3_layout", order: 1 },
  { id: "workbench.action.editorLayoutTwoColumns", title: "Two Columns", menu: "menubar/view/editorLayout", group: "3_layout", order: 2 },
  { id: "workbench.action.editorLayoutThreeColumns", title: "Three Columns", menu: "menubar/view/editorLayout", group: "3_layout", order: 3 },
  { id: "workbench.action.editorLayoutTwoRows", title: "Two Rows", menu: "menubar/view/editorLayout", group: "3_layout", order: 4 },
  { id: "workbench.action.editorLayoutThreeRows", title: "Three Rows", menu: "menubar/view/editorLayout", group: "3_layout", order: 5 },
  { id: "workbench.action.editorLayoutTwoByTwoGrid", title: "Grid (2x2)", menu: "menubar/view/editorLayout", group: "3_layout", order: 6 },
  { id: "workbench.action.editorLayoutTwoRowsRight", title: "Two Rows Right", menu: "menubar/view/editorLayout", group: "3_layout", order: 7 },
  { id: "workbench.action.editorLayoutTwoColumnsBottom", title: "Two Columns Bottom", menu: "menubar/view/editorLayout", group: "3_layout", order: 8 },
  { id: "workbench.action.toggleEditorGroupLayout", title: "Flip Layout", menu: "menubar/view/editorLayout", group: "4_flip", order: 1, keybinding: "shift+alt+0" },
  // Go
  { id: "workbench.action.navigateBack", title: "Back", menu: MenuId.MenubarGoMenu, group: "1_back", order: 1, keybinding: "alt+left" },
  { id: "workbench.action.navigateForward", title: "Forward", menu: MenuId.MenubarGoMenu, group: "1_back", order: 2, keybinding: "alt+right" },
  { id: "workbench.action.navigateToLastEditLocation", title: "Last Edit Location", menu: MenuId.MenubarGoMenu, group: "1_back", order: 3, keybinding: "ctrlcmd+m ctrlcmd+q" },
  { id: "workbench.action.showAllSymbols", title: "Go to Symbol in Workspace...", menu: MenuId.MenubarGoMenu, group: "3_global_nav", order: 2, keybinding: "ctrlcmd+t" },
  { id: "workbench.action.gotoSymbol", title: "Go to Symbol in Editor...", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 1, keybinding: "ctrlcmd+shift+o" },
  { id: "editor.action.revealDefinition", title: "Go to Definition", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 2, keybinding: "f12" },
  { id: "editor.action.revealDeclaration", title: "Go to Declaration", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 3 },
  { id: "editor.action.goToTypeDefinition", title: "Go to Type Definition", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 4 },
  { id: "editor.action.goToImplementation", title: "Go to Implementations", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 5, keybinding: "ctrlcmd+f12" },
  { id: "workbench.action.addSymbolToCurrentChat", title: "Add Symbol to Current Chat", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 6, keybinding: "ctrlcmd+l" },
  { id: "editor.action.goToReferences", title: "Go to References", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 7, keybinding: "shift+f12" },
  { id: "workbench.action.addSymbolToNewChat", title: "Add Symbol to New Chat", menu: MenuId.MenubarGoMenu, group: "4_symbol_nav", order: 8, keybinding: "ctrlcmd+shift+l" },
  { id: "workbench.action.editor.nextChange", title: "Next Change", menu: MenuId.MenubarGoMenu, group: "7_change_nav", order: 1, keybinding: "alt+f3" },
  { id: "workbench.action.editor.previousChange", title: "Previous Change", menu: MenuId.MenubarGoMenu, group: "7_change_nav", order: 2, keybinding: "shift+alt+f3" },
  // Go > Switch Editor (Next/Previous Editor are wired by the editor contribution)
  { id: "workbench.action.openNextRecentlyUsedEditor", title: "Next Used Editor", menu: "menubar/go/switchEditor", group: "2_used", order: 1 },
  { id: "workbench.action.openPreviousRecentlyUsedEditor", title: "Previous Used Editor", menu: "menubar/go/switchEditor", group: "2_used", order: 2 },
  { id: "workbench.action.nextEditorInGroup", title: "Next Editor in Group", menu: "menubar/go/switchEditor", group: "3_group", order: 1, keybinding: "ctrlcmd+m ctrlcmd+pagedown" },
  { id: "workbench.action.previousEditorInGroup", title: "Previous Editor in Group", menu: "menubar/go/switchEditor", group: "3_group", order: 2, keybinding: "ctrlcmd+m ctrlcmd+pageup" },
  { id: "workbench.action.openNextRecentlyUsedEditorInGroup", title: "Next Used Editor in Group", menu: "menubar/go/switchEditor", group: "4_used_group", order: 1 },
  { id: "workbench.action.openPreviousRecentlyUsedEditorInGroup", title: "Previous Used Editor in Group", menu: "menubar/go/switchEditor", group: "4_used_group", order: 2 },
  // Go > Switch Group
  { id: "workbench.action.focusFirstEditorGroup", title: "Group 1", menu: "menubar/go/switchGroup", group: "1_by_index", order: 1, keybinding: "ctrlcmd+1" },
  { id: "workbench.action.focusSecondEditorGroup", title: "Group 2", menu: "menubar/go/switchGroup", group: "1_by_index", order: 2, keybinding: "ctrlcmd+2" },
  { id: "workbench.action.focusThirdEditorGroup", title: "Group 3", menu: "menubar/go/switchGroup", group: "1_by_index", order: 3, keybinding: "ctrlcmd+3" },
  { id: "workbench.action.focusFourthEditorGroup", title: "Group 4", menu: "menubar/go/switchGroup", group: "1_by_index", order: 4 },
  { id: "workbench.action.focusFifthEditorGroup", title: "Group 5", menu: "menubar/go/switchGroup", group: "1_by_index", order: 5 },
  { id: "workbench.action.focusNextGroup", title: "Next Group", menu: "menubar/go/switchGroup", group: "2_nav", order: 1 },
  { id: "workbench.action.focusPreviousGroup", title: "Previous Group", menu: "menubar/go/switchGroup", group: "2_nav", order: 2 },
  { id: "workbench.action.focusLeftGroup", title: "Group Left", menu: "menubar/go/switchGroup", group: "3_directional", order: 1, keybinding: "ctrlcmd+m ctrlcmd+left" },
  { id: "workbench.action.focusRightGroup", title: "Group Right", menu: "menubar/go/switchGroup", group: "3_directional", order: 2, keybinding: "ctrlcmd+m ctrlcmd+right" },
  { id: "workbench.action.focusAboveGroup", title: "Group Above", menu: "menubar/go/switchGroup", group: "3_directional", order: 3, keybinding: "ctrlcmd+m ctrlcmd+up" },
  { id: "workbench.action.focusBelowGroup", title: "Group Below", menu: "menubar/go/switchGroup", group: "3_directional", order: 4, keybinding: "ctrlcmd+m ctrlcmd+down" },
  // Run
  { id: "workbench.action.debug.start", title: "Start Debugging", menu: MenuId.MenubarRunMenu, group: "1_debug", order: 1, keybinding: "f5" },
  { id: "workbench.action.debug.run", title: "Run Without Debugging", menu: MenuId.MenubarRunMenu, group: "1_debug", order: 2, keybinding: "ctrlcmd+f5" },
  { id: "workbench.action.debug.stop", title: "Stop Debugging", menu: MenuId.MenubarRunMenu, group: "1_debug", order: 3, keybinding: "shift+f5" },
  { id: "workbench.action.debug.restart", title: "Restart Debugging", menu: MenuId.MenubarRunMenu, group: "1_debug", order: 4, keybinding: "ctrlcmd+shift+f5" },
  { id: "workbench.action.debug.configure", title: "Open Configurations", menu: MenuId.MenubarRunMenu, group: "2_configurations", order: 1 },
  { id: "workbench.action.debug.addConfiguration", title: "Add Configuration...", menu: MenuId.MenubarRunMenu, group: "2_configurations", order: 2 },
  { id: "workbench.action.debug.stepOver", title: "Step Over", menu: MenuId.MenubarRunMenu, group: "3_step", order: 1, keybinding: "f10" },
  { id: "workbench.action.debug.stepInto", title: "Step Into", menu: MenuId.MenubarRunMenu, group: "3_step", order: 2, keybinding: "f11" },
  { id: "workbench.action.debug.stepOut", title: "Step Out", menu: MenuId.MenubarRunMenu, group: "3_step", order: 3, keybinding: "shift+f11" },
  { id: "workbench.action.debug.continue", title: "Continue", menu: MenuId.MenubarRunMenu, group: "3_step", order: 4, keybinding: "f5" },
  { id: "editor.debug.action.toggleBreakpoint", title: "Toggle Breakpoint", menu: MenuId.MenubarRunMenu, group: "4_breakpoints", order: 1, keybinding: "f9" },
  { id: "editor.debug.action.enableAllBreakpoints", title: "Enable All Breakpoints", menu: MenuId.MenubarRunMenu, group: "5_breakpoints", order: 1 },
  { id: "editor.debug.action.disableAllBreakpoints", title: "Disable All Breakpoints", menu: MenuId.MenubarRunMenu, group: "5_breakpoints", order: 2 },
  { id: "editor.debug.action.removeAllBreakpoints", title: "Remove All Breakpoints", menu: MenuId.MenubarRunMenu, group: "5_breakpoints", order: 3 },
  { id: "workbench.action.debug.installAdditionalDebuggers", title: "Install Additional Debuggers...", menu: MenuId.MenubarRunMenu, group: "6_install", order: 1 },
  // Run > New Breakpoint
  { id: "editor.debug.action.conditionalBreakpoint", title: "Conditional Breakpoint...", menu: "menubar/run/newBreakpoint", group: "1_breakpoints", order: 1 },
  { id: "editor.debug.action.editBreakpoint", title: "Edit Breakpoint", menu: "menubar/run/newBreakpoint", group: "1_breakpoints", order: 2 },
  { id: "editor.debug.action.toggleInlineBreakpoint", title: "Inline Breakpoint", menu: "menubar/run/newBreakpoint", group: "1_breakpoints", order: 3, keybinding: "shift+f9" },
  { id: "editor.debug.action.addFunctionBreakpoint", title: "Function Breakpoint...", menu: "menubar/run/newBreakpoint", group: "1_breakpoints", order: 4 },
  { id: "editor.debug.action.addLogpoint", title: "Logpoint...", menu: "menubar/run/newBreakpoint", group: "1_breakpoints", order: 5 },
  { id: "editor.debug.action.addTriggeredBreakpoint", title: "Triggered Breakpoint...", menu: "menubar/run/newBreakpoint", group: "1_breakpoints", order: 6 },
  // Terminal
  { id: "workbench.action.terminal.new", title: "New Terminal", menu: MenuId.MenubarTerminalMenu, group: "1_terminal", order: 1, keybinding: "ctrlcmd+shift+`" },
  { id: "workbench.action.terminal.split", title: "Split Terminal", menu: MenuId.MenubarTerminalMenu, group: "1_terminal", order: 2, keybinding: "ctrlcmd+shift+5" },
  { id: "workbench.action.tasks.runTask", title: "Run Task...", menu: MenuId.MenubarTerminalMenu, group: "2_tasks", order: 1 },
  { id: "workbench.action.tasks.build", title: "Run Build Task...", menu: MenuId.MenubarTerminalMenu, group: "2_tasks", order: 2, keybinding: "ctrlcmd+shift+b" },
  { id: "workbench.action.terminal.runActiveFile", title: "Run Active File", menu: MenuId.MenubarTerminalMenu, group: "2_tasks", order: 3 },
  { id: "workbench.action.terminal.runSelectedText", title: "Run Selected Text", menu: MenuId.MenubarTerminalMenu, group: "2_tasks", order: 4 },
  { id: "workbench.action.tasks.showTasks", title: "Show Running Tasks...", menu: MenuId.MenubarTerminalMenu, group: "3_running", order: 1 },
  { id: "workbench.action.tasks.restartTask", title: "Restart Running Task...", menu: MenuId.MenubarTerminalMenu, group: "3_running", order: 2 },
  { id: "workbench.action.tasks.terminateTask", title: "Terminate Task...", menu: MenuId.MenubarTerminalMenu, group: "3_running", order: 3 },
  { id: "workbench.action.tasks.configureTaskRunner", title: "Configure Tasks...", menu: MenuId.MenubarTerminalMenu, group: "4_configure", order: 1 },
  { id: "workbench.action.tasks.configureDefaultBuildTask", title: "Configure Default Build Task...", menu: MenuId.MenubarTerminalMenu, group: "4_configure", order: 2 },
  // Help (Show All Commands is the wired showCommands' second placement, registered by the quickinput contribution)
  { id: "update.showCurrentReleaseNotes", title: "Show Release Notes", menu: MenuId.MenubarHelpMenu, group: "2_notes", order: 1 },
  { id: "workbench.action.openIssueReporter", title: "Report Issue", menu: MenuId.MenubarHelpMenu, group: "3_feedback", order: 1, keybinding: "ctrlcmd+m ctrlcmd+g" },
  { id: "workbench.action.giveFeedback", title: "Give Feedback...", menu: MenuId.MenubarHelpMenu, group: "3_feedback", order: 2 },
  { id: "workbench.action.openLicenseUrl", title: "View License", menu: MenuId.MenubarHelpMenu, group: "4_license", order: 1 },
  { id: "workbench.action.toggleDevTools", title: "Toggle Developer Tools", menu: MenuId.MenubarHelpMenu, group: "5_devtools", order: 1 },
  { id: "workbench.action.openProcessExplorer", title: "Open Process Explorer", menu: MenuId.MenubarHelpMenu, group: "5_devtools", order: 2 },
] satisfies readonly StubRow[];

for (const row of stubRows) {
  const result: Result<IDisposable, ParseError> = registerAction({
    id: row.id,
    title: row.title,
    precondition: "false",
    toggled: row.toggled,
    keybinding: row.keybinding === undefined ? undefined : { keybinding: row.keybinding },
    menu: [{ id: row.menu, group: row.group, order: row.order }],
    run: () => {},
  });
  if (!result.ok) {
    console.error(`stub action '${row.id}': ${result.error.message}`);
  }
}

// Second placements: one command, two menus. Appended directly so each
// row keeps its own label - the action registry's menu entries omit the
// title, and the two placements' labels differ (Tasks) or the row sorts
// under another menu's group (Extensions). Same pattern as the
// quickinput contribution's Show All Commands row.
appendMenuItem("menubar/file/preferences", {
  command: "workbench.view.extensions",
  title: "Extensions",
  group: "1_settings",
  order: 3,
});
appendMenuItem("menubar/file/preferences", {
  command: "workbench.action.tasks.configureTaskRunner",
  title: "Tasks",
  group: "1_settings",
  order: 6,
});
