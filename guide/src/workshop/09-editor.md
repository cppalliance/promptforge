# The Editor

You have granted folders and you can browse them in the Workshop tree. This chapter teaches you to open the files those folders contain, edit them, and save them safely. The editor is where reading the agent's work and making your own changes happen, and it is built so you never lose text or silently overwrite someone else's.

## Opening a file

To open a file, click it in the Workshop tree. The file opens in its own tabbed editor panel in the main zone, with one panel per file. The tab title shows the file's base name rather than its full path.

You can open a text file from a granted folder and see its full contents, up to a 1 MiB size limit. The editor targets source text, not media. A larger read fails with an error that states the byte limit. Binary files cannot be edited; the attempt is rejected with "file is binary, not text". Files that are not valid UTF-8 are rejected with "file is not utf-8 text".

The editing surface is a CodeMirror-based text editor. Syntax highlighting is chosen automatically from the file extension: JavaScript, TypeScript, JSX, TSX, Python, Rust, JSON, Markdown, YAML, and TOML. Files with unknown or missing extensions open as plain text with no highlighting mode. You can search within the open document using the editor's built-in search panel, styled to match the application's dark theme.

## Editing and saving

Edit the text as you would in any code editor. A dot marker appears in the tab title when the document has unsaved changes, and clears when the document is clean again.

To save the active editor, press Ctrl+S. The shortcut does nothing when no editor is active. To close the active editor, press Ctrl+W; a clean panel closes immediately. To move between open editors, press Ctrl+Tab to cycle forward and Ctrl+Shift+Tab to cycle in reverse, wrapping around at the ends.

You can create a new file inside a granted folder by saving to a path that does not exist yet.

Saves are atomic. You never see a half-written file or a leftover temporary file after a save. A crash or power loss during a save leaves either the old contents or the new, never a truncation. You also never lose unsaved typing to a slow save: edits made while a save write is still in flight remain marked as unsaved after the save completes. Triggering a second save while one is in flight does nothing, so you cannot stack overlapping writes.

Load and save failures appear as an alert bar above the editor. The newest error replaces the previous one. The editor also warns when a panel opens with no file path.

## Conflicts

When you save a file that changed on disk since it was read, the save is refused with a conflict instead of silently overwriting. Each save carries the version token from the previous successful write, so the editor never silently overwrites a file that changed elsewhere. You get a "File changed on disk" dialog with two choices:

- Reload discards the editor's text and loads the on-disk text.
- Overwrite writes your changes over the file on disk, re-reading the fresh token first so the write succeeds.

## Closing with unsaved changes

Closing a panel with unsaved changes opens an "Unsaved changes" dialog with three choices:

- Save writes the file and closes the panel.
- Discard abandons your changes and closes the panel.
- Cancel returns you to the editor.

A failed or conflicted save leaves the panel open. The panel closes only after a successful write.

## Dialogs and read-only mode

Modal prompts, such as the editor's conflict and close prompts and the tree's Add Folder prompt, appear as a themed dialog box overlaid on the panel you are working in, dimming the rest of that panel. Dialog behavior is consistent across panels:

- You read a title and a message line at the top of each prompt.
- Prompts can show a labeled single-line text field.
- When a dialog opens, focus moves into it, landing in the text field or on the first button.
- Destructive actions are styled as danger buttons.
- Value-dependent buttons stay disabled until you type something.
- Enter inside the text field submits the dialog through its primary button.
- Escape dismisses the dialog without taking any action.
- Tab and Shift+Tab cycle focus within the dialog's controls and cannot escape to the panel behind it.
- When the dialog closes, focus returns to the element that had focus before the dialog opened.
- Re-invoking an already-open dialog does nothing.

You can toggle the editor between editable and read-only without losing the document, the undo history, or the view state. When the workspace reloads a file from the server, the reload lands in place as one marked transaction instead of an editor rebuild: you keep undo history, selection, and scroll position, and you can undo back across the reload. A reloaded file arrives clean and is not flagged as an unsaved change.

You can now open, edit, and save workspace files with confidence. The final chapter teaches you to keep the application current and tuned: updates, the About dialog, and the Gateway Config panel.

