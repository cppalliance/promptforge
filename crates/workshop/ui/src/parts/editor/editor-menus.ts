// The editor's own menu id. It lives in a file of its own so the eager
// editor.contribution.ts (which fills the menu) and the lazy
// editor-panel.ts (which opens it at the pointer) share one id without
// either importing the other. MenuId is a plain string, so the id needs no
// entry in the platform's list.

/** The menu a right-click on the editor opens: Cut, Copy, Paste, Command Palette.... */
export const EDITOR_CONTEXT_MENU = "editor/context";
