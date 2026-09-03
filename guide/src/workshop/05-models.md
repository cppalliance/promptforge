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

