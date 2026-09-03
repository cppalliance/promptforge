# The Workspace

You can converse with an agent. This chapter teaches you to give the agent files to work on. The Workshop never roams your disk on its own: you grant it access to specific folders, and the Workshop tree panel on the left shows you exactly what you have granted. By the end you will know how to grant folders, browse them, and take access away.

## Granting a folder

The fastest way to grant a folder is drag and drop. In the desktop application, drop a folder onto the window and it becomes a workspace root. Dropping a single file grants the application access to the file's parent folder instead of just the file. On Windows you can drop files or folders straight from Explorer, and the application receives the real OS paths of the dropped items. Each successfully dropped path is confirmed on the status bar with a message naming the path. When one dropped path cannot be opened, the status bar shows an error for that path and the remaining dropped paths are still added.

- Dropping a file onto the window never by itself gives the application access to the file's bytes. The page grants each dropped path through the workspace API first.
- Dropping files onto the window never navigates the page away from your session. In-page drags such as panel tab drags keep their normal behavior; only drags carrying OS files are intercepted.

You can also add a folder without dragging. Click the header "+" button labeled "Add Folder to Workspace...", or right-click empty space in the panel and choose the same item. In the desktop application you pick a folder through the native folder picker. In a plain browser you type the path into an "Add Folder to Workspace" dialog. The drop-to-grant feature is desktop only; in a plain browser, dropping files keeps the normal HTML drag/drop behavior of reading file contents and never grants workspace access.

The outcome of adding or removing a folder is always announced on the status bar, as a success or an error. Grants registered through any session are visible to every open session immediately, and open panels such as the Workshop tree refresh automatically to show new grants.

Folder grants last only for the current session. They are held in memory and are not saved to the profile.

## Browsing the tree

The Workshop tree lists the granted workspace roots and browses one directory at a time. When no folder is selected, the panel shows the granted folders as the top level of the tree. When no folders are granted, you see the hint "Drop a folder onto the window to browse it here."

Each granted folder row shows the folder's own name rather than the full path, with the full path available as the row tooltip. A drive root shows its path. Directory listings show folders before files, each group sorted alphabetically by name. Each entry carries its name, full path, kind (directory or file), byte size, and modification time. Browsing is paths only: the tree lists names and never reads file contents.

To browse:

1. Click a directory's chevron to expand it. Click again to collapse it.
2. Click a file to open it in the editor zone. The Editor chapter covers what happens next.

Your expansion state and fetched listings persist for the session. Closing and reopening the Workshop panel restores the tree as it was left. A directory load failure appears as an error row inside the affected list, exposed to assistive technology as an alert. Pressing Ctrl+Shift+F activates the file tree and moves keyboard focus into it, even while the tree is empty.

A granted folder that has been deleted from disk still appears in the panel, flagged as missing so you can clean it up: a struck-through name in the danger color plus a "missing" text label.

## Confined access

The grant boundary is enforced, not cosmetic. You cannot open, list, or save any path outside the granted folders; the application refuses with a "path is outside every granted root" error.

The refusal messages are precise about what went wrong:

- Paths containing `..` are refused before any disk access, however they were encoded, with "path contains a forbidden component". On Windows, file names containing a colon are refused.
- A path that is not a regular file fails with "path is not a file".
- A tree listing for something that is not a directory fails with "path is not a directory".
- A missing path reports "path does not exist".

Nested grants are independent. Revoking a parent folder's grant leaves a separately granted child intact, and files under the child stay reachable.

Dropped paths keep their native spelling, including backslashes, spaces, and Unicode characters. Any Windows verbatim prefix is removed. On older WebView2 runtimes, Explorer drops degrade gracefully instead of failing the application.

## Revoking a grant

To take access away:

1. Right-click the root row of the granted folder.
2. Choose "Remove from Workspace".

Files under the removed folder lose access on their next operation. Removing an unknown root reports "path is not a granted root". A root deleted from disk stays removable, so you can always clean up a missing entry.

You can now grant folders and browse them. The next chapter teaches the editor, where you open and change the files those folders contain.

