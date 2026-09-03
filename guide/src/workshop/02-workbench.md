# The Workbench

You know how the Workshop starts and what its window is. This chapter teaches you how that window is organized: the regions it is divided into, the panels that live in those regions, and how to arrange them to fit the way you work. Everything you do in the Workshop happens inside a panel, so learning the layout once pays off in every later chapter.

## The three zones

The Workshop window is a dock area divided into three named zones, rendered in the Cursor Dark visual theme:

- The left zone holds the workspace tree.
- The main zone holds document editors.
- The right zone holds the agent session.

Each kind of panel has a default zone it opens in until you move it. The Workshop tree opens on the left, editors open in the main zone, and the agent session opens on the right. On a fresh start you see two panels: the Workshop tree docked on the left, titled "Workshop", and the Agent Session panel docked on the right. The main zone stays empty until you open a document.

Below the dock area, a permanent full-width status bar runs along the bottom of the window. It is not part of the dock and is never saved as part of the layout.

## The title bar

Across the top of the window sits a custom title bar. It shows the PromptForge program icon, carries the five application menus (File, Edit, Model, Window, Help), and leaves an empty center region you can grab. On Windows this bar replaces the native window frame; macOS and Linux keep their decorated windows. The bar is always shown, even when you run the Workshop in a plain browser, because it carries the application menus.

To operate the window from the title bar:

- Drag the empty center region with the primary mouse button to move the window.
- Double-click the same region to toggle between maximized and restored.
- Click the Minimize, Maximize, or Close control at the right end to operate the window.

The controls appear in the Windows-standard order: Minimize, Maximize, Close. The maximize control swaps its glyph and label between "Maximize" and "Restore" to match the window's current state, including changes made by Windows Snap or by drag-resizing. The window reopens at its previous size and position on the next launch. The native window controls appear only in the desktop application. In a plain browser the control cluster is hidden, because there is no native window for the commands to act on; the menus still work.

## Zooming the interface

You can scale the whole interface to a comfortable size. Zoom applies uniformly to the whole window, so the chat, the editor, and every other surface scale together.

- Press Ctrl+= to zoom in one step. Ctrl+Shift+= also zooms in.
- Press Ctrl+- to zoom out one step.
- Press Ctrl+0 to reset to 100%.

Zoom changes in fixed steps of 10 percent, clamped between 50% and 200%. Your chosen level persists across sessions and is re-applied on every boot. A missing, corrupt, or out-of-range saved value leaves the default 100% in place. Zoom keeps working even when storage is blocked, such as in private mode; only the persistence is skipped. In a plain browser, zoom uses CSS zoom instead of native window zoom.

## Panels

A panel is one unit of content in the dock: the Workshop tree, an editor, an agent session, or the Gateway Config panel. Every panel renders a normal chip tab, so tabs are always visible even when a panel is alone in its group.

A few rules govern how panels open:

- Reopening a panel that is already open brings it to focus instead of opening a duplicate.
- Each open document gets its own editor tab keyed by its file path. The same file never opens twice.
- Each agent session gets its own panel keyed by its instance id, so multiple agent sessions can be open side by side.
- Panel kinds other than editors and agent sessions are singletons. Only one of each can be open at a time.

Editor tabs are titled with the file's base name rather than its full path. Panel tabs update their displayed title when the panel's title changes. If an unknown panel is ever requested, you see a labelled placeholder instead of a broken dock.

You can close an Agent Session tab with the close button on the tab. Right-clicking an Agent Session tab opens a context menu with "Close" and "Close Others" actions.

- Press Ctrl+B to close the Workshop tree panel. Press Ctrl+B again to reopen it.

## Rearranging the layout

The workbench is never locked. You can drag panels to rearrange the layout at any time.

When you move a panel to another zone, the application remembers that choice and reopens the panel in your chosen zone next time. Moving a panel back to its default zone clears the remembered override, so the panel follows its type's normal placement again.

Closing every panel in a zone collapses that zone. Opening a new panel into it rebuilds the zone on its own side of the dock. A rebuilt main zone regrows beside the left zone when possible, otherwise beside the right zone, so the layout keeps its familiar shape.

## Layout persistence

The panel layout persists across sessions. Layout changes save automatically shortly after you move, resize, open, or close panels. There is no manual save step.

If the saved layout is missing, corrupt, or from an older version of the application, the Workshop discards it and boots the known-good default layout: the Workshop tree anchored left and the agent session open right. You can never lose the Workshop tree or the agent session. Both panels are restored on every boot even if a stale saved layout dropped them, and the Workshop tree's tab has no close button.

A few small behaviors keep the workbench predictable. Drag-and-drop of panels inside the application always works, because the application avoids registering an OS-level drop target that would break in-page dragging. The browser's native right-click context menu is suppressed inside the application, so right-clicks always produce Workshop menus.

