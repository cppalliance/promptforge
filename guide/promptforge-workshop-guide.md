# The Workshop

---

# The Application

This chapter teaches you what the Workshop desktop application is, how to install and start it, and what you see the first time its window opens. Everything else in this guide happens inside this one window, so it is worth a few minutes to understand what the application is made of and how it boots before you touch any feature.

## What the Workshop is

PromptForge Workshop is a desktop application for Windows, macOS, and Linux. You launch one program named Workshop. That program boots a small server inside itself and then opens a single window titled "PromptForge". The window shows the Workshop interface, which the built-in server serves on your own machine. There is no separate web server to install and no files to download before the interface can appear; the interface ships bundled inside the application.

The Workshop talks to a PromptForge gateway. The gateway is the part of the system that supplies the model catalog, the profiles, and the model rounds that power chat. The gateway runs as its own program, separate from the Workshop window: the application's built-in server attaches to a running gateway over HTTP, so closing the window never unloads the gateway or its loaded models. The window opens at 1024 by 768 pixels the first time, and it remembers its size, position, and maximized state across launches.

The application shows the PromptForge program icon in its custom title bar.

## Installing and starting the Workshop

You receive the application as a Windows installer, a macOS disk image, a Debian package, or a Linux AppImage, depending on your platform. On Windows the installer silently includes the webview runtime the application needs, so there is no separate setup step.

To start the application, launch it the way you launch any installed program on your platform. If you work from a source checkout instead, one command builds and starts it:

````
cargo run -p workshop
````

To check which version you have without starting anything, run:

````
promptforge-workshop --version
````

This prints the version and exits. It does not start the server and it does not open a window.

The installed application can also check for updates and update itself. After startup it automatically checks the latest GitHub Release, and it installs only cryptographically verified updates.

You can also run the Workshop's server on its own and use the interface in an ordinary browser. In that mode you open the chat UI at `http://127.0.0.1:7910/`. The browser session works like the desktop window for almost everything; the few differences, such as native window controls and Explorer drag-and-drop, are called out in the chapters that cover them.

## The first launch

The first time you start the Workshop, the application prepares everything it needs before you see a window. Follow what happens:

1. The application looks for its boot configuration.
2. It starts its server inside its own process and waits until the server accepts connections.
3. It waits for the interface to answer a health check, up to 15 seconds.
4. Only then does the window open.

You never see a window before the interface is ready, and the interface never opens against a dead server. If the server does not answer in time, the error message names the health endpoint and how long the application waited. If startup fails for any reason, the application prints the full error chain and exits with a failure code instead of opening a broken window.

Only one instance of the Workshop runs at a time. If you launch it again while it is already running, the existing window comes into focus instead of a second copy opening. When you close the window, the application shuts its built-in server down cleanly and exits; the gateway is a separate program and keeps running. To stop the gateway together with the window, use the quit command instead: Quit PromptForge and Gateway on the application menu, or Ctrl+Q (Cmd+Q on macOS). When the Workshop is attached to a gateway on another machine, the command reads Quit PromptForge and stops only the window - a client never stops a shared gateway. In-flight connections get a 5-second grace window, so a held chat session or a stuck request cannot hang the shutdown. The interface listens on an OS-assigned loopback port, so another program holding a port can never block startup.

The Workshop also keeps working when parts of its environment fail. The interface still loads when the gateway is unreachable, so a gateway outage never prevents the application from opening. If microphone setup fails at startup, you keep working and only voice input stays unavailable. On Windows, if the bridge to Explorer fails to attach, the application keeps running and loses only Explorer drag-and-drop and the microphone grant.

## The gateway configuration

The gateway owns its own boot config, `gateway.toml`, and the Workshop never reads it. On the gateway's first run - when no config exists anywhere it searches - the gateway writes a default `gateway.toml` into `%USERPROFILE%\.promptforge\` and prints a message telling you where it wrote the file. It also creates `profiles\default.toml` beside it, and it never overwrites an existing `profiles\default.toml`. The generated config boots the gateway into the `default` profile.

The generated config is a single editable TOML file with a header that invites edits. Two properties of the generated file are worth knowing:

- The gateway is secured with a freshly generated random bearer key, so no two installs share a key.
- The gateway listens on the loopback address only, on an OS-assigned port. It is not reachable from other machines, and the Workshop learns the port from the connection file the gateway writes.

A `gateway.toml` carried over from an older version may declare a `[workshop]` section. It still parses: the gateway logs a deprecation warning, its `bind` and `open_browser` settings do nothing (the Workshop's server now lives inside the desktop application), and only the `[workshop.stt]` capture tuning still applies.

At run time the gateway also downloads the pinned voice runtime matched to your machine (CUDA on Windows, Metal on Apple Silicon, CPU on the other supported targets), plus the managed `llama-server`. You make no build-time choices for this.

## The Workshop configuration

You configure the Workshop through a TOML file named `workshop.toml`. The application searches three places in order: beside the executable, the current directory, and `~/.promptforge/workshop.toml`. The first file found wins. Every field is optional and the defaults are built in. With no file anywhere, the application keeps its state in `~/.promptforge/` and attaches to the gateway through its connection file. The application never writes the file, and the standalone server's `workbench.toml` fallback does not apply to it.

The keys you are most likely to set:

- `gateway.base_url` points the Workshop at a PromptForge gateway the connection file cannot see, such as one on another machine. When the value is empty, the Workshop attaches to a locally running gateway through its connection file, and with no gateway running, startup fails with an error that names both remedies.
- `gateway.api_key` supplies the bearer key for the gateway API. An empty key sends no `Authorization` header, which is right for a gateway running with authentication disabled.
- `server.bind` is honored only by the standalone `workshop-server` binary. The desktop application owns its listener and always binds `127.0.0.1` on an OS-assigned port.
- `server.state_dir` chooses where the Workshop keeps persistent state. Agent session event logs live under `state_dir/sessions/`, and the per-profile model memory is written there. It defaults to the config file's own directory.
- `agents.path` chooses which directory of `.lua` agent programs is launchable. The default is `agents/` beside the config file. A missing directory offers no agents; that is a state, not an error.

String values support `${VAR}` environment interpolation, so you can keep secrets out of the file. A literal dollar sign is written `$$`. An unset variable interpolates to the empty string instead of failing startup.

The configuration is strict about mistakes, so you find out about problems immediately. A config without a `[gateway]` section fails to load. Unknown keys or sections are a startup error, such as a leftover `[voice]` section from an older version. Error messages name the offending file, and a malformed `${...}` interpolation gives a clear error. A browser launch failure, by contrast, is only logged as a warning; it never stops the server.

## Working with your operating system

The Workshop is a desktop citizen, not just a web page in a frame.

You can drag files from your operating system and drop them into the application to attach them. You can open native file and folder picker dialogs from the Workshop. When you click a link to an external website, it opens in your system browser while the Workshop window stays on its own page. Links between pages served by the Workshop itself load inside the application window.

One protection is worth understanding early: a link to any other local server, even one on the same port spelled `localhost` or `[::1]`, opens in the system browser. No other program on your machine gets the application's desktop features.

## Safety and limits

The Workshop is built so that only you, on your own machine, can reach it.

The window loads its interface only from the local machine, never from a remote address. The Workshop refuses any request a browser marks as coming from another website, and it only answers requests addressed to a loopback host. Requests that change things must declare a JSON body. The live socket that carries chat only upgrades for the Workshop's own loopback origin or a native client.

Nothing hangs forever. A stalled request is answered with a timeout error instead of freezing: ordinary routes give up after 10 seconds, and routes that relay a call to the gateway allow up to 35 seconds so a stalled gateway surfaces as a meaningful failure. Live socket sessions are never cut off by a request deadline. A gateway that is down or wedged fails fast in the interface: connections give up after 5 seconds and ordinary requests after 30 seconds.

Startup also cleans up after previous runs. Leftover temporary files in the state directory are swept away on boot, so a crash during a previous save never leaves residue that affects the next launch.

You now know what the application is, how it starts, and what it connects to. The next chapter opens the window and walks through its regions.

---

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

---

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

---

# The Status Bar

You know the window, its panels, and its menus. This chapter teaches you the status bar, the permanent full-width footer at the bottom of the window. The status bar is how the Workshop tells you what it is doing whenever something takes noticeable time: startup phases, gateway round trips, dictation and transcription, and model downloads. Learning to read it means you always know whether the application is idle, working, or stuck, and why.

## Reading the bar

The status bar shows a short label as its text. When startup finishes and nothing is happening, the resting state reads "Ready". Hover over the bar to see a longer description of the current status as a tooltip. Failures appear as errors, visually distinct from ordinary status updates: the text switches to red. Long status text truncates with an ellipsis instead of overflowing the bar, and numbers use fixed-width digits so values do not jitter as they change. The bar announces its updates to assistive technology.

During startup you see a "Connecting to gateway" update that names the gateway base URL being contacted. When startup finishes and nothing is happening, the bar returns to "Ready".

## The right slot: progress bar and lights

The right end of the bar holds one of two things, never both at once. While an operation reports progress, a progress bar fills the slot. Otherwise the slot holds the indicator lights. The slot swaps as a unit.

When an activity can report how far along it is, you see determinate progress: units completed so far against units expected in total. A model download, for example, shows its label, the file name as the description, and a current-of-total count. Gateway-side work such as model downloads and profile switches renders on the Workshop status bar through the same progress display as local operations.

When no progress is showing, two small lights sit in the slot:

- The activity LED pulses green while output tokens arrive and amber while a model turn is thinking. It also tells gateway traffic (green) from dictation activity (amber). Green wins when both coincide. The thinking LED stays lit for the whole thinking period, not just a brief flash. Pulses fade in fast and decay slowly, so a stream of activity reads as one continuous glow.
- The recording LED lights up red while the microphone is recording.

Both LEDs sit dark when the application is idle. The recording LED sits one LED-width to the left of the activity LED. When a chat is aborted, the activity LED goes dark immediately, even though no final server status arrives for that chat. When an error status arrives, the activity LED goes dark at once and does not light again on its own.

## Gateway connectivity

The status bar is where you watch the gateway connection. The Workshop probes the gateway's health endpoint and treats a transport failure, a slow answer, or a non-success status as unreachable. Each probe is bounded at 2 seconds. The Workshop opens and works normally whether or not the gateway has ever answered; only gateway calls wait.

- When the gateway stops answering, the bar announces "Gateway unreachable" with the explanation "the gateway does not answer its health probe". Calls to the gateway are not attempted while it is down.
- When the gateway returns, the bar announces "Connected to gateway". The model catalog refreshes by itself, because a gateway that was down may serve a different catalog.

You are notified only when reachability changes. A steady state never re-announces itself. While the gateway is reachable, the Workshop checks its health every 5 seconds, so a recovery is detected within about 5 seconds. While the gateway is down, retries use a jittered, escalating delay: starting at about 5 seconds, doubling per attempt, and never exceeding one minute. A gateway that accepts connections but never answers keeps the escalated schedule, because only useful work resets it. After roughly a full day of continuous outage, the Workshop stops probing and shows "Gateway reconnect stopped" with the advice "the reconnect budget is exhausted; restart the workshop to retry".

When a gateway call fails in transport, you see the gateway's own summary line as the error message. Every failure you hit surfaces as a short plain-language message near the status text. Production builds show no internal detail; debug builds append the underlying cause chain after the message.

Gateway progress appears on the status bar only while the gateway is reachable. When the gateway becomes unreachable the progress entry disappears instead of going stale. After a reconnect the progress resumes with a single fresh entry.

## Live delivery and reconnection

The application holds one persistent live connection to the server. Status updates, the model catalog, and menu state arrive in the interface as they happen, with no manual refresh. The interface boots with its status bar, catalog, and menu state already populated; there are no loading round trips. Snapshots are pushed on every connect and resent on reconnect, and the newest status update is retained and replayed to late-connecting sessions, so if you reconnect you immediately see the current status. A late-joining session gets a status line recomputed from the current probe, not a stale retained announcement; if real work is in progress, such as a model download or a chat, that work's status frame replays as-is.

When the connection to the server drops, the status bar returns to a neutral "Reconnecting..." state. The application reconnects automatically: retries start at a one-second wait and double on each failure, capped at 30 seconds. The application connects over a secure socket automatically when the page is served over HTTPS, and a plain socket otherwise.

Locally-originated messages such as dictation errors appear in the status bar too, and are replaced by the next server status update.

## Why the bar stays calm

The status bar is engineered not to flicker, so what you see is always meaningful:

- An operation that finishes in under one second never disturbs the status bar.
- Once the progress indicator appears, it stays visible for at least half a second.
- The bar never steps backward, even when a new operation starts while the previous bar is still on screen. Back-to-back operations share one continuous bar.
- When an operation has several sub-tasks, the bar shows a single weighted aggregate and the label names the sub-task that is still unfinished.
- Internal instrumentation never reaches the screen. Debug-level updates never change the status bar text or tooltip, though they still pulse the activity LED; only info and error severities are displayed.
- If updates arrive faster than the interface can draw them, the display skips ahead to the newest snapshot instead of lagging behind.
- Updates that arrive while the application is still starting are held and replayed in arrival order once the interface is ready. The holding queue is bounded at 32 pushes with the oldest dropped when full, and if the connection drops before the interface is ready, the queued messages are cleared.

You can now read everything the application tells you about its state. The next chapter teaches you to choose what the application runs: models and profiles.

---

# Models and Profiles

You can read the status bar, so you can tell when the application is ready. This chapter teaches you to choose what the application runs: the model that answers your chats, and the profile that decides which models exist. By the end you will be able to pick a model, understand when chat is ready, and switch profiles with confidence.

## The catalog

The Workshop does not invent its model list. The catalog comes from the configured gateway, which serves it at `GET /v1/models`. The Workshop relays the catalog verbatim, including upstream error bodies, so what you see matches the gateway's answer. Each model lists its id and owner, with an optional description. Each push replaces the previous list in full.

Every connected session receives each catalog update, so all open sessions show the same current list. A session that connects later receives the current catalog immediately. The catalog also refreshes automatically every time the gateway comes back after an outage, because a gateway that was down may serve a different catalog. A boot-time catalog failure heals itself this way. A failed, declined, or malformed catalog answer is logged and skipped rather than pushed, so your pickers never lose a usable list.

While the Workshop fetches the catalog, the status bar shows "Loading models...". When the gateway is known to be down, the request is refused immediately with the message "Gateway unreachable". A non-success answer shows "Gateway error: <status>". A failed connection shows "Connection lost" with the underlying detail. A successful fetch returns the status area to idle.

## Picking a model

You pick a model from the Model menu in the title bar. The menu lists every catalog model as a checkable radio row with the selected one checked, and each model's description appears as a tooltip on its row. When the catalog is empty, the menu shows a disabled "No models available" row.

The agent toolbar offers a second way to pick: a pill-shaped button that displays the id of the currently selected model. To use it:

1. Click the pill button. A dropdown menu opens listing every model in the catalog.
2. Click a model. It becomes the current model.

When no model is selected, the pill shows the label "Select model". When the catalog is empty, the dropdown shows a single inert "No models available" row. Hovering the button shows the current model's description as a tooltip.

One current model selection is shared by every Agent tab and the title-bar Model menu, so the chosen model stays consistent across the whole application. Your pick is sent to the server as a command, and the on-screen selection changes only when the server confirms it. The button label updates only after that confirmation, never optimistically on click. A catalog refresh never silently changes which model is selected, and selection indicators update only on a real change, so the Model menu and Agent tabs do not flicker when the server re-confirms the same model. Picking an unknown model id is refused with an error message, and the previous selection stays in place.

If a refreshed catalog no longer contains the selected model, the Model menu clears the selection and chat becomes unavailable until you pick again.

## When chat is ready

Chat input is enabled only when all of these hold: the catalog has models, a model is selected, no profile switch is in flight, and the gateway is reachable. The server computes this readiness; the interface never derives it.

On startup and after every reconnect, the application restores the remembered model for the active profile, falling back to the first catalog model when the remembered one is gone. A fresh boot against a live gateway lands ready to chat with no manual pick. While the gateway is unreachable, chat input stays disabled even with a model selected. Your chosen model survives the outage; only chat readiness flips, and the selection is still in place when the gateway returns.

If a model selection cannot be sent because the connection is down, the status bar shows an error naming the model and the cause: "Could not select <model>: the workshop socket is down".

## Profiles

A profile is a named configuration on the gateway that decides which models are available. The Workshop shows you the list of profiles the gateway offers and which profile is currently active, read from the gateway. You can see the Model menu's full state at a glance: every profile, the active profile, any profile switch in flight, and the selected model. A gateway without profile support shows an empty profile list instead of an error or stale names.

To switch the active profile:

1. Open the Model menu.
2. Find the Profiles section at the bottom. It appears only when the gateway offers two or more profiles. The active profile is checked.
3. Click the profile you want.

The switch proceeds through three labeled stages shown in order with determinate counts: "Loading profile..." (1 of 3), "Stopping models..." (2 of 3), "Starting models..." (3 of 3). The status bar names the profile being switched to while progress is shown. A profile switch can run for minutes while model weights load into VRAM; the progress stream has no deadline, so you keep seeing progress for as long as it takes.

While a switch runs, the menu shows a pending state and chat input is disabled. Only one profile switch runs at a time; starting a second switch while one is in flight is refused with an error. A profile switch you started runs to completion even if you disconnect while it is in progress.

When a profile switch completes, the application selects the model last used on that profile, or the first catalog model when none is remembered. Chat becomes ready again and the status bar returns to idle. When a profile switch fails, you see a "Profile switch failed" notification carrying the gateway's own error message. The previous profile stays active and keeps serving, though its local models may already be stopped, and the selected model and chat readiness are restored. After any profile switch, succeeded or failed, the profile list and model catalog are refreshed, so the menu reflects the gateway's real state. A garbled progress update during a switch degrades that one update but never the switch itself; you still see the final outcome. If the connection is down when you try to switch, a local error appears on the status bar: "Could not switch to <name>: the workshop socket is down".

The application remembers the selected model per profile and restores it across restarts. The memory lives in a `workshop-state.json` file in the server's state directory. A missing, unreadable, or corrupt memory file never blocks startup; the application starts with no memory and selects the first catalog model.

## The model cache

You can trigger a download of a model blob into the gateway's cache and watch cumulative progress until the blob is ready or the download fails. When the requested blob is already cached, you get an immediate ready answer instead of a download. The cache feature is meaningful only in the standard local deployment, where the Workshop and the gateway run on the same machine and share the filesystem.

Before the application receives its first state from the server, you see an empty workbench: no profiles, no active profile, no selected model, and chat gated off. Every server push refreshes the Model menu and chat gating, even when nothing changed, so the display never goes stale.

You now have a model selected and chat ready. The next chapter teaches the chat surface itself.

---

# The Chat Surface

You have a model selected and chat is ready. This chapter teaches you the chat surface itself: how to send a prompt, how to read the transcript, and how to steer a session once it is running. Chat is the heart of the Workshop, and everything here builds directly on the Models and Profiles chapter.

## Your first message

The Agent Session panel on the right side of the window is where you talk to the selected model. Chat always runs as a live agent session, not a one-shot buffered request. Every reply streams through the open session, which opens instantly and stays open for the whole session.

The default chat is a transparent pass-through with no added system prompt and no tools. Your messages go to the model currently selected in the interface. A fresh install always offers this working built-in chat agent, even when there is no agents directory at all. Later you can add your own agents by dropping `.lua` files into the agents directory; each file appears as a launchable agent under its file-stem name in a sorted list, and a newly added agent file shows up in the agent list on the next connect, without a restart. Placing a `chat.lua` file in the agents directory shadows the built-in one, so you can replace the default chat with your own program. An existing `chat.lua` that cannot be read surfaces its error instead of silently serving the embedded source.

To send your first message:

1. Click into the input box at the bottom of the Agent Session panel. The placeholder reads "Plan, Build, / for skills, @ for context".
2. Type your message.
3. Press Enter.

Enter sends the prompt. Shift+Enter inserts a newline without sending. If you use a CJK input method, an Enter that commits an IME composition never sends, so you can confirm candidates safely.

Sending delivers exactly the text you typed, never trimmed. An empty box sends nothing. A failed send keeps the text for retry. A successful send clears the box. The box grows and shrinks with what you type, within a minimum and maximum height (about 36px to 200px), and scrolls past the maximum.

The prompt box and send button enable only while the agent is asking for input. Otherwise the box is read-only and send is disabled.

A push-to-talk microphone button sits beside the send button. It stays visible in every state, and when dictation cannot start, a click names the blocker on the status bar. The Voice Input chapter covers dictation.

## Reading the transcript

The session reads as a scrolling feed of rows, one row per transcript entry, with each kind of entry styled distinctly. The feed scrolls itself to the newest entry whenever it repaints. New rows are announced to assistive technology as they arrive; settled history is never rebuilt or re-announced during streaming.

Your own messages appear under a muted "You" label as plain text, right-aligned as bubbles. Text you send is never interpreted as markup, so pasted or typed HTML cannot inject formatting or scripts.

Agent replies render as formatted Markdown with a muted line above naming the model that produced the reply. Replies and reasoning that are still streaming carry a visible pending style and a blinking caret at the live tail. While a reply streams, you see the answer text arrive chunk by chunk. The status bar shows "Running agent turn" while the agent thinks, "Streaming response..." while the reply streams, and "Ready" when the turn completes. The model's reasoning streams live on its own side channel, separate from the answer text, and appears in a collapsible block titled "Reasoning" or "Reasoning (model)". It stays open while it streams and collapses once it settles.

Tool calls appear as collapsible cards with a clickable header. The header shows the tool's name (or a generic "Tool call" / "Tool calls" label), a count badge for multi-call batches, and a status dot. A card opens on its own while the call runs and closes when the result arrives. A card you opened by hand stays open. Each call's arguments render as syntax-highlighted JSON. The result appears as a preformatted block labeled with the id of the call it answers. A batch that cannot be parsed still renders as raw text instead of vanishing.

Errors appear inline in the transcript with a visible "Error: " label, never by color alone. A message that could not be sent because the connection is down appears as a local notice: "The message was not sent: the agent socket is down."

You can observe per-reply model metrics such as token usage and generation speed attached to the assistant's replies. The log records which model produced each entry, per-reply token usage (prompt, completion, cached, and reasoning tokens), and per-reply timings (time to first token, generation speed in tokens per second, and end-to-end latency).

## Mentions and the composer extras

You can mention files with @ and pick them from a typeahead popup that opens next to the cursor. The list filters its entries by case-insensitive substring match against the text typed after the @. While the popup is open, ArrowUp and ArrowDown move the highlight through the suggestion list with wraparound, and Enter inserts the highlighted item instead of sending the message. Clicking a row inserts that file without moving focus out of the editor. Escape dismisses the popup. A query with no matches hides the popup.

Each referenced file appears as an inline pill inside the prompt editor, with a file icon and the file's label. The pill behaves as a single unit, not editable text. Clicking the X button on the pill removes the whole mention. The suggestion list currently offers three canned file entries (README.md, src/main.ts, Cargo.toml) as a stand-in until the workspace file index exists.

## The agent toolbar

A toolbar above the input bar groups the mode chip, the model picker, and a context-usage ring in one row.

The mode chip lets you choose among five agent interaction modes: Agent, Plan, Debug, Multitask, and Ask. The chip starts in Agent mode. Click it and pick a mode; the chip's icon and label update immediately and the change is announced to the rest of the application. Re-picking the current mode produces no change and no event.

The context ring is a small 16px gauge showing how much of the model's context window the current session has used. The arc fills in proportion to the percentage used. The ring reads 0 percent until real usage data exists, and readings are clamped between 0 and 100. Assistive technology hears it announced as "Context usage" with the current percentage.

The model picker in the toolbar is the pill button from the Models and Profiles chapter; it shares the same selection as the title-bar Model menu.

## Sessions that survive

A session is more durable than its connection. Agent sessions survive a dropped connection. The socket reconnects on its own and reattaches to the same session. The server replays the persisted event log from the beginning, and a per-client cursor drops duplicates, so you see each event exactly once and in order. Every unanswered prompt is re-announced in the order it was asked.

You can also attach to an already running session by its session id, resuming where that session stands. Sessions outlive sockets.

Your run history is recorded as a durable event log that survives restarts. Each session's conversation persists to a JSONL transcript file named after the session id under the sessions state directory. The log format is versioned, so session logs saved on disk keep loading after every application update. A damaged, truncated, or incompatible history file is refused with a clear error instead of showing a wrong or partial history. You can return to a previous run and continue it: the saved history is restored with its original ordering, and new events append to the same record. If saving the log to disk fails, the run keeps working and nothing you see is lost; the failure is logged as a warning and saving retries on later events.

The chat shows both sides of the conversation back to the model each turn, rebuilding the message list from the recorded user and agent messages. The conversation accumulates turn over turn, and what you typed reaches the model byte-exact, with newlines, quotes, and unicode preserved. Selecting another model takes effect on the next turn, and each reply is attributed to the model that produced it. A relaunch over retained or reloaded history resumes the conversation exactly where it stood.

## Cancelling and failing gracefully

You can cancel a running turn. Cancellation is a stop reason, never an error. Pending prompts close as cancelled, and the relaunched agent returns to waiting over its retained history. The chat is immediately usable again.

The chat survives a transport failure: the session surfaces the failure and returns to waiting for the next message. When a single model round fails, you see an error message naming the agent; the agent survives the failure and returns to waiting for input. When a run fails outright, you see an "Agent failed" notification carrying the error text. If stream chunks are dropped on a slow connection, the completed transcript event repairs the text. Late chunks that arrive after a cancel are discarded, so you never see duplicate or orphaned streaming text.

Closing a session ends the agent run for good with no relaunch. The saved transcript stays on disk.

## When the agent asks you a question

Some agent programs pause and ask for input. When an agent program needs input, the Workshop presents a prompt in the session's input box and waits for you to type an answer. The input box stays pinned to that request until it is answered. Each prompt accepts exactly one answer, and your typed answer reaches the agent byte-exact as typed, preserving newlines, quotes, braces, backslashes, and non-ASCII characters.

Cancelling a turn while a prompt is pending dismisses that prompt, so the input box is never left stuck on a dead question. A prompt that dies unresolved is explicitly cancelled on screen, never silently abandoned. A pending prompt survives a lost connection: on reconnect, every unanswered prompt is shown again in the order it was asked, and a stale prompt vanishes. You can answer a prompt that was asked while the socket was down; the answer is delivered normally once the session is back.

## The agent panel

You work with one agent session per panel. Opening a new panel starts a fresh session. Closing the panel ends the session and releases its connection. The panel automatically launches the "chat" agent when the server reports available agents, falling back to the first available agent when "chat" is not present. You can open additional agent sessions from the Agents menu (New Agent) or the Workshop menu (Open Agent Session). Each new session gets its own panel in the right zone. Agent windows are modal: one window serves one session at a time, and trying to open a second session in the same window is refused with an explanation.

While the panel has no active session, you see a launchable-agent menu labeled "Agents" for assistive technology, with the lead line "Launch an agent to start a session." There is one button per discovered agent, labeled with the agent's name; clicking it launches a session. When no agents are discovered, you see the message "No agents discovered." After you launch an agent, every launch button disables until the server answers, preventing a double launch. A refused launch shows the server's error message and re-enables the buttons for another try. When the agent socket is down, you see "The agent socket is down; it reconnects by itself. Try again shortly." and no launch is sent. The whole menu disappears once the session acknowledgment arrives, replaced by the session surface. Starting or reattaching to a session clears any pending input prompt; a same-session reattach keeps the transcript, and a new session starts the transcript fresh.

## What chat content can contain

Model-authored chat content renders as Markdown: headings, bold, italic, inline code, lists, blockquotes, tables, links, and images. Fenced code blocks are syntax-highlighted in the application's dark theme in twelve languages: bash, css, html, javascript, json, lua, markdown, python, rust, toml, typescript, and yaml. A code block in an unrecognized language renders as a plain code block, and if highlighting fails to initialize, code blocks still render as plain preformatted text.

You can size an image embedded in chat content by appending a ` =WxH` or ` =Wx` dimension suffix to the image source. Links show a tooltip on hover that defaults to the link URL.

Model-authored markup is sanitized before display. Scripts, inline event handlers, and dangerous URLs such as javascript: links are stripped. Tool results render as plain text, so markup inside a result can never execute.

Launching an agent is refused when the gateway settings cannot produce a usable model client. The error tells you to check `gateway.base_url` and `gateway.api_key` in `workshop.toml`. The rest of the Workshop keeps serving.

You can now hold a full conversation, steer it, and recover from anything that interrupts it. The next chapter teaches you to speak your prompts instead of typing them.

---

# Voice Input

You can type prompts into the chat surface. This chapter teaches you to speak them instead. Dictation uses a push-to-talk microphone button beside the send button, and the transcript lands in the prompt exactly as if you had typed it. If voice is not available on your machine, this chapter also teaches you how to tell and why.

Dictation is temporarily unavailable in the desktop application: the app no longer merges the gateway's `/stt` speech socket, which migrates into the Workshop's own server in a later change. Until then the microphone button names the blocker on the status bar. This chapter describes the feature as it behaves where the socket is served.

## Dictating a prompt

To dictate into the chat input:

1. Click the microphone button beside the send button. Its tooltip reads "Push to talk".
2. Speak your message.
3. Click the microphone button again to stop. The tooltip now reads "Stop recording".

While you speak, you see live transcription as a growing committed prefix plus a tentative tail. When you stop, the assembled final transcript replaces the interim text and focus returns to the input. After you stop, the input stays locked until the final transcript arrives; a slow transcription is allowed up to two minutes.

Dictation splices the transcript into the current selection, behaving like typing at the cursor. Newlines in the transcript become line breaks. Dictating over a selection replaces the selection outright. Consecutive takes compose, because each take captures the cursor position fresh at record start. You never see stale transcription text from a previous take: takes are numbered per connection, and frames from a superseded take are discarded.

While a take records, the input locks against typing and shows a recording ring, so the insertion geometry cannot be disturbed. You can still press Enter to send what the box shows. Sending during a take sends the visible text, interim transcript included, and discards the take. Discarding a live take, for example by closing the tab or starting a new session, restores the pre-take text and unlocks the input. An empty take tells you no speech was detected, with the number of captured audio frames.

The status bar shows a red recording LED while the microphone is capturing, and the mic button shows a solid danger-colored fill with a matching ring while recording.

## When the mic does nothing

The mic stays visible and clickable in every state. Dictation is gated on a capability check and on a pending input wait: the application asks the server what dictation can do here and treats any failure of that check as blocked. Clicking the mic while dictation cannot start names the blocker on the status bar instead of silently doing nothing:

- "Dictation is still checking what this server can do; try again in a moment."
- "Dictation needs a GPU this server doesn't have."
- "No speech models are provisioned in the active profile."
- "The agent isn't asking for input; the mic opens when it does."

Failures during dictation are named too. Microphone permission denial or capture failure is named on the status bar. A dropped dictation connection is reported on the status bar, including drops before the final transcript lands. A server error message during a take is shown verbatim on the status bar and ends the take. A browser without microphone, audio, or WebSocket support is told "Dictation is not available in this browser."

Under the hood, the Workshop serves a speech-to-text socket endpoint at `/stt`. Dictation streams your speech to it continuously as mono audio blocks while you talk. Microphone capture applies echo cancellation and noise suppression, and the audio is resampled to 16 kHz before it is sent for transcription.

## Microphone permission on each platform

Each platform handles the microphone grant differently:

- On Windows, the application grants the microphone permission automatically. You are never interrupted by a microphone permission prompt. Every other permission kind keeps the normal browser behavior.
- On Linux, the application turns on media capture in its webview and grants microphone and camera capture requests automatically. Other permission requests, such as notifications and geolocation, remain denied by default.
- On macOS, the application holds the audio-input entitlement that permits microphone capture for local dictation. The system permission prompt explains: "PromptForge uses the microphone you select for local voice dictation."

If microphone setup fails at startup, you can keep working in the application and only voice input stays unavailable.

## Voice configuration

Voice input comes pre-tuned with a 15-second transcription window and a 500 ms interval, set in the `[workshop.stt]` section of the boot config:

````
[workshop.stt]
window_seconds = 15
interval_ms = 500
````

You can add a `vocabulary` list of domain terms to bias recognition:

````
vocabulary = ["MCP", "GGUF", "Lua"]
````

First run provisions two recommended speech-to-text models: `whisper-base-en` for interim results and `whisper-small-en` for final results. They download from Hugging Face with pinned sha256 checksums and stated VRAM requirements of 1.0 GB and 2.0 GB. The generated configuration boots the gateway into a profile named `default` that activates both provisioned whisper models.

You can now speak or type your prompts. The next chapter teaches you to give the agent files to work on by granting folders to the workspace.

---

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

---

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

---

# Updates and Configuration

You can operate the whole application: the window, the panels, the menus, the status bar, models, chat, voice, the workspace, and the editor. This final chapter teaches you to keep the Workshop current and tuned: the update flow, the About dialog, and the embedded Gateway Config panel.

## Keeping the Workshop up to date

The installed application automatically checks the latest GitHub Release shortly after startup and installs only cryptographically verified updates. Downloaded updates are verified against a pinned public key before installation, so tampered updates are rejected. The automatic check runs on the desktop application only, and update checks give up after 30 seconds rather than hanging. On Windows, updates install passively, applying with minimal interruption to your session.

Platform notes:

- On Linux the update flow is available only when running as an AppImage. Package-managed installations show the update flow as unsupported and never contact the update endpoint.
- In a plain browser session the update flow stays inert.
- Nightly builds do not produce updater artifacts, so a nightly install does not receive automatic in-app updates.

When an update is available, you see a banner floating at the bottom-right corner of the window, above the status bar. The banner shows the new version number and a one-line summary of the release notes. You have two choices:

- Click "Remind me later" to dismiss the banner and bring the prompt back later.
- Click "Update now" to start the update immediately.

While an update downloads, installs, or restarts, a full-screen modal overlay takes over the window. You watch download progress as a percentage and a progress bar, with bytes received against the total size. After the download finishes, the application installs the update and restarts itself.

When an update download or install fails, you see the failure reason and can dismiss the overlay with a Close button to return to the application. You can expand an "Update log" section in the overlay to read the raw log lines produced during the update. When the application is already up to date, the update state reports that no update is available. When an update check fails, you see an error message.

## The About dialog

Open Help > About PromptForge to see the About dialog. It names the product, the application version, and the license, shown as "License: BSL-1.0". A development build shows the version "dev" instead of a release number.

The About dialog is also where you trigger an update check manually. The update button reflects the state:

- "Desktop updates unavailable" in a browser.
- "Updates are managed by your package manager" on package-managed installs.
- "Checking for updates..." while a check runs.
- "Show update <version>" when an update is ready.
- "Retry update check" after a failed check.

The About dialog traps keyboard focus: Tab and Shift+Tab cycle between its buttons and never leave the modal. You can dismiss it with the Escape key or the Close button, and focus returns to the element that opened it. Only one About dialog can be open at a time.

## The Gateway Config panel

You can view and change gateway configuration without leaving the Workshop, in the Gateway Config panel. The panel opens in the main zone through the application's Gateway Config command, titled "Gateway Config". Opening it a second time focuses the existing panel instead of opening a duplicate, and you can close it from its tab's close action.

The panel embeds the gateway's configuration web interface, served same-origin through the Workshop at the `/gateway/config/` route in panel mode. It opens in the dark theme on the local gateway view. From the panel you can:

- View the gateway's current configuration.
- Edit and save gateway configuration and environment values.
- Apply or revert pending configuration changes, and see whether the configuration has unsaved edits or changes waiting to be applied.
- Search and browse Hugging Face models.
- View gateway status, system information, model information, chat templates, environment, and orphaned files.
- View the downloaded model cache and delete a cached model to free disk space.
- Trigger the gateway's reveal action.

Panel actions are announced on the Workshop status bar: "Gateway configuration applied", "Gateway configuration changes reverted", and "Gateway download started". Long-running panel operations such as cache downloads and profile switches can stream for minutes without being cut off by a timeout. When the gateway is unreachable, the panel reports the failure instead of hanging.

You never handle the gateway access key. The Workshop server attaches the bearer key on the server side of every forwarded panel request. Neither the Workshop page nor the embedded config panel ever sees it, and the key is never written to logs. The panel's API requests go through an allowlisted proxy; anything outside the configuration surface is refused, including chat completions, progress subscriptions, health checks, and direct cache uploads. Deleting a cached model is allowed only by its 64-character lowercase hex digest. Requests with malformed or absolute targets are refused locally with a forbidden status before anything leaves the application. The panel is reachable only from your own machine, never from the local network, and the embedded configuration interface runs in a restricted sandbox limited to running scripts within the same origin.

## Reskinning the interface

If you build the Workshop from source, you can reskin the entire interface by editing CSS custom properties in the `:root` block of `ui/style.css`. Every color, spacing step, radius, font, scrollbar metric, and the status bar's LED and progress effect is a custom property there. To reskin without editing the shipped stylesheet, add a `<link>` after `/style.css` in `ui/index.html` and redeclare any variable on `:root`. Later declarations win the cascade. Focus on menus and controls is shown through state backgrounds, opacity, or underlines, never through outline rings or focus boxes.

You have completed the tour. You can install and start the Workshop, read its window and status bar, pick models and switch profiles, converse with an agent by keyboard or voice, grant folders, edit files, and keep the application current and configured.
