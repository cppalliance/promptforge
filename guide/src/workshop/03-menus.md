# Menus and Commands

You know the window's regions and panels. This chapter teaches you the command surface that sits on top of them: the five menus in the title bar, the keyboard shortcuts, and how menus behave. Once you know where the commands live, every later chapter can simply name a command and you will know where to find it.

## The five menus

The title bar carries five menus: File, Edit, Model, Window, and Help. Click a menu button to open its popover. Here is what each menu holds.

The File menu:

- New Agent starts a fresh agent session; it opens or focuses the agent-session panel. New Agent is the only new-conversation command. There is no New Chat.
- Close Window closes the window, also with Alt+F4.

The Edit menu runs Undo, Redo, Cut, Copy, Paste, and Select All with the standard shortcuts Ctrl+Z, Ctrl+Y, Ctrl+X, Ctrl+C, Ctrl+V, and Ctrl+A. After an Edit command runs, focus returns to the field that had it.

The Window menu:

- Workshop Panel toggles the Workshop panel tree, also with Ctrl+B.
- Gateway Config opens or focuses the gateway configuration panel. It sits directly after Workshop Panel.
- New Agent opens or focuses the agent-session panel. It sits directly after Gateway Config.
- Zoom In, Zoom Out, and Reset Zoom zoom the interface, with shortcuts Ctrl+=, Ctrl+-, and Ctrl+0. Ctrl+Shift+= also zooms in.
- Minimize and Maximize/Restore operate the window. These menu commands do exactly what the visible title bar buttons do.

The Model menu lists every catalog model as a checkable radio row with the selected one checked. Each model's description appears as a tooltip on its row. When the catalog is empty, the Model menu shows a disabled "No models available" row. A Profiles section at the bottom of the Model menu switches the gateway profile; it appears only when the gateway offers two or more profiles, and the active profile is checked. The Models and Profiles chapter covers this menu in depth.

Help > About PromptForge opens the About dialog, which also shows the desktop update state. The Updates and Configuration chapter covers it.

## Keyboard shortcuts

Beyond the menu shortcuts, the application binds a small fixed set of keys:

- Ctrl+S saves the active editor. The shortcut does nothing when no editor is active.
- Ctrl+W closes the active editor and prompts when there are unsaved changes.
- Ctrl+B toggles the Workshop tree panel open and closed.
- Ctrl+Tab cycles through the open editors and Ctrl+Shift+Tab cycles in reverse, wrapping around at the ends.
- Ctrl+Shift+F opens or activates the Workshop tree and moves keyboard focus into it.

The bindings are fixed. You cannot customize them, and there are no multi-key chords. Only plain Ctrl combinations are bound; combinations with Alt or Meta are left untouched. Unbound key combinations fall through to the browser and the editor, so typing, selection, clipboard, undo/redo, and in-file find keep their normal behavior. Inside the desktop application the browser's built-in shortcuts are disabled, so the application's own key handling never races them.

## How menus behave

Menus in the Workshop follow the desktop conventions you already know, with a few details worth learning once.

Edit menu commands are enabled only when an editable element (a text input, textarea, or contenteditable element) holds focus. They act on the element that was focused before the menu opened. A disabled command cannot run and does not close the menu.

You can navigate open menus with the keyboard. ArrowDown and ArrowUp move between rows with wraparound. ArrowRight and ArrowLeft switch menus. Enter runs the focused row. Escape closes the menu and returns focus to its button. While any menu is open, hovering another menu button switches to it. Hovering alone opens nothing when no menu is open. An open menu closes when you click anywhere outside it or when the window loses focus.

Menu rows show the label on the left and the shortcut hint on the right in muted, smaller text. Disabled rows are muted and do not react to hover. Thin separator lines group related rows. Checkable rows keep a fixed-width check column so labels stay aligned.

The Model menu is live. It rebuilds its rows from the catalog every time it opens, and again whenever a workbench snapshot arrives while it stays open, so check marks move without reopening the menu. Clicking a model row sends the selection, and the check mark moves only when the server confirms the new selection. Keyboard focus survives a live rebuild of the open menu: focus stays on the equivalent row and falls back to the first row if the focused row disappears. While a profile switch is loading, every Model menu row disables, and the switch target shows a pending "..." mark in place of its check until the server confirms. The still-active profile keeps its checkmark.

The same menus work in a plain browser. Only the native window commands (Minimize, Maximize/Restore, Close Window) do nothing there, because no desktop bridge carries them.

## Context menus

Some panels, such as the Workshop tree, open a context menu of action items from a trigger element. Context menus share one set of behaviors:

- Activating the same trigger a second time closes the menu. At most one menu is open at a time.
- Items can carry an icon next to the label, a check mark for the selected choice, and a danger style for destructive actions.
- A right-click invocation opens the menu at the pointer position. The menu flips above the trigger or right-aligns when it would overflow the window.
- Escape dismisses the menu and returns focus to the trigger. ArrowUp, ArrowDown, Home, and End move through the items. Tab closes the menu.
- Activating an item runs its action and closes the menu immediately.
- The trigger announces its expanded state to assistive technology.

Panels and chat use one consistent set of small inline outline icons. The trash icon deletes an item, the folder-plus icon creates a folder, the microphone icon starts voice input, and the send icon sends the message. The icons are sized 15 or 16 pixels and drawn in the surrounding text color, so they stay legible across themes.

You can now reach every command the application offers. The next chapter teaches the status bar, which is how the application reports what it is doing while you work.

