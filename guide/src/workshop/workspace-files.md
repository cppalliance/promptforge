# Workspace Files

You can grant folders and browse them. This chapter teaches you to keep that arrangement: a workspace file remembers your granted folders and your window layout, so they come back the next time you launch. By the end you will know how to save a workspace, open one, duplicate one, and what a workspace file does and does not hold.

## What a workspace file is

A workspace is a single file with the extension `.pfwork`. It is an ordinary file you can see in your file manager, copy, move, back up, and delete. Inside, it is a small embedded database; you never need to look inside it, but if you are curious, any Turso or SQLite inspector opens it.

A workspace file holds your arrangement of that workspace:

- The granted folders, in the order you granted them. The folders themselves are not copied; the file remembers their paths.
- The window's size, position, and maximized state.
- The panel layout, which folders are expanded in the tree, and the list of editors you have closed (for Reopen Closed Editor).

That is all. Your files stay where they are on disk, and your agent sessions are unaffected. The workspace is a bag of preferences, not a project archive. The "What persists" section below spells out what lives in the workspace and what follows you between workspaces.

The workspace commands use native file dialogs, so they are desktop only. In a plain browser the three File menu rows are disabled.

## Ephemeral until saved

When you launch the Workshop for the first time, or open no workspace, you are working in an ephemeral workspace. Everything works exactly as in the previous chapter, and nothing is remembered: folder grants and the window layout last only for the current session. This is the state the previous chapter described when it said grants are held in memory.

To start remembering, save the workspace once. From then on there is nothing more to save.

## Saving a workspace

1. Open the File menu.
2. Choose "Save Workspace As...".
3. In the save dialog, pick a folder and a name. The dialog suggests `Untitled.pfwork` for an ephemeral workspace and the current workspace's name otherwise. The `.pfwork` extension is added for you if you leave it off.

The Workshop creates exactly one file at the path you chose. It does not create a folder around it. The current grants and window layout are written into it, the Workshop switches to it, and the file appears under File > Open Recent.

From now on every change is saved as it happens. Grant a folder and it lands in the file; remove one and it leaves the file; move or resize the window and the new geometry is saved a moment after you stop dragging, and once more when you close the window. There is no unsaved state, no dirty marker, and no Save command, because the file is a live mirror of what you see.

If you save while a workspace is already open, you get a second file with the same grants and layout and the Workshop switches to the new one. The original stays where it is, unchanged from that point on.

## Reopening at launch

The Workshop remembers which workspace was open when you last quit. When you launch it again, that workspace is reopened before the window appears: your granted folders are back in the tree and the window opens at its saved size and position.

If the file has been moved, deleted, or damaged since, the Workshop starts with an ephemeral workspace instead and notes the reason in its log. Launch never fails because of a workspace file.

## Opening a workspace

1. Open the File menu.
2. Choose "Open Workspace from File...".
3. Pick a `.pfwork` file in the file dialog.

The file's grants replace your current grants entirely, the tree refreshes, and the window moves to the file's saved geometry. Opening a workspace is the same trust gesture as dropping a folder onto the window: you are deliberately granting the Workshop access to the folders the file names, and every restored folder is visible in the tree. A granted folder that no longer exists on disk still appears, flagged as missing, so you can remove it.

A file that is not a PromptForge workspace is refused with a message naming the file, and a workspace saved by a newer version of the Workshop is refused with the version it needs. In both cases nothing changes: your current grants stay, and the refused file is not touched.

Recently opened and saved workspaces are listed under File > Open Recent alongside recently opened files, so you can see which workspaces you have used. In this version the list is a record only: to open one of them, use "Open Workspace from File..." and pick the file.

## Duplicating a workspace

1. Open the File menu.
2. Choose "Duplicate Workspace...".
3. Pick a folder and a name for the copy.

The Workshop makes a complete, independent copy of the current workspace and switches to it. Changes you make afterwards go to the copy; the original is untouched, and vice versa. If no workspace file is open, there is nothing to copy, so Duplicate behaves exactly like Save Workspace As: a new file is created from the current grants and layout.

Save Workspace As and Duplicate Workspace look alike today because a workspace is one file. They differ in what travels. Save As means "my preferences under a new name": only the workspace file is written. Duplicate means "the whole world comes along": in future versions, when a workspace has grown companion folders beside it (see below), Duplicate copies them too and Save As leaves them with the original.

## Companion folders

A workspace file may in future gain sibling folders beside it, created only when there is something to put in them: `agents/` for agent databases, `runs/` for saved runs, and so on. They are plain folders with plain names, so their relationship to the workspace file is self-evident in your file manager. Nothing in the current version creates them.

Because siblings are named for their role rather than for the workspace, two `.pfwork` files in the same folder would share them. Keep one workspace per folder. The Workshop does not stop you from doing otherwise, but you will find the arrangement confusing later.

## What persists

The Workshop remembers your interface state in two buckets, split by whether the state belongs to a workspace or to you.

The workspace bucket lives in the `.pfwork` file and comes back whenever that workspace is open:

- The granted folders and the window geometry, as described above.
- The panel layout: which panels are open, where they sit, and their sizes.
- Which folders are expanded in the Workshop tree. Restored folders load their listings on demand, so an expanded folder shows its children.
- The closed-editor list, so Reopen Closed Editor works across launches.

The user bucket lives in the Workshop's own state directory and follows you from workspace to workspace:

- Editor toggles: word wrap, rendered whitespace, control characters, column selection.
- The zoom level.
- Recent files and recent workspaces under File > Open Recent.
- The command palette's history.

Both buckets save as you go. There is no Save command for either. While a workspace is ephemeral, the workspace bucket has nowhere to go and lasts only for the session; the user bucket saves regardless.

Opening a workspace applies its bucket in place of what you see. The live layout is replaced by the file's layout, and every open editor is disposed, including editors with unsaved text, so save your work before you open another workspace. The tree collapses to the file's expanded folders. A restored agent panel is a panel, not a conversation: it starts a fresh session, and your earlier sessions stay in the state directory as before. Saving a workspace under a new name copies the live layout, tree, and closed-editor list into the new file so it opens as you left it.

If either bucket cannot be read or written, the Workshop starts from defaults for that bucket, notes the reason in its log, and keeps working; nothing you do in the interface is blocked by a persistence failure.

## What is not in the workspace

- Your files. The workspace remembers paths, not contents.
- Agent sessions and their transcripts. Those live in the Workshop's own state directory, as before.
- Anything from before this version. Existing state is not imported; save a workspace to start one.
- Editor toggles, zoom, recent files, and command history. Those are yours, not the workspace's, and stay the same as you move between workspaces.

You can now save, open, and duplicate workspaces, and you know which of your settings travel with a workspace and which follow you. The next chapter teaches the editor, where you open and change the files those folders contain.
