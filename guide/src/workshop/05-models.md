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

A profile is a named checklist on the gateway that decides which local and speech models it loads at boot. Remote models are always available; the profile governs what runs on the gateway's own machine. The Workshop shows you the list of profiles the gateway offers and which profile is currently active, read from the gateway. You can see the Model menu's full state at a glance: every profile, the active profile, any profile selection in progress, and the selected model. A gateway without profile support shows an empty profile list instead of an error or stale names.

The gateway loads its local models once, when it starts, so changing the profile means restarting the gateway. When the gateway is a sidecar the Workshop launched and supervises, the Workshop performs that restart for you. To select a profile:

1. Open the Model menu.
2. Find the Profiles section at the bottom. It appears whenever the gateway defines at least one profile. "No profile" is the first entry, and the active profile is checked.
3. Click the profile you want, or "No profile" to run remote models only.

The selection runs a sequence of up to three labeled stages shown in order with determinate counts: "Selecting profile..." (1 of 3), "Restarting gateway..." (2 of 3), "Loading models..." (3 of 3). The status bar names the profile being selected while progress is shown. The first stage persists the selection on the gateway. When the gateway is already running the chosen profile, the sequence stops there and the menu settles at once. Otherwise, for a supervised sidecar, the Workshop asks the gateway to shut down and waits up to 90 seconds for its relaunched replacement to come up serving the chosen profile; the replacement's boot then loads the profile's models, which can take minutes while weights load into VRAM.

When the gateway is one you configured on another machine, the Workshop never stops it. The selection persists on that gateway and the status bar reads "Profile selected" with a notice that you must restart the gateway by hand to load it; the running profile stays active until you do.

While a selection runs, the menu shows a pending state and chat input is disabled. Only one selection runs at a time; starting a second while one is in flight is refused with an error.

When a selection completes, the application selects the model last used on that profile, or the first catalog model when none is remembered. Chat becomes ready again and the status bar returns to idle. When a selection fails, you see a "Profile switch failed" notification with the gateway's own error message; if the gateway still serves, the selected model and chat readiness are restored. A sidecar that was shut down and did not return in time reports "gateway did not return after restart", and the Workshop's supervisor keeps looking for it and repopulates the menu when it appears. After any selection that leaves a gateway serving, the profile list and model catalog are refreshed, so the menu reflects the gateway's real state. If the connection is down when you try to select, a local error appears on the status bar: "Could not switch to <name>: the workshop socket is down".

The application remembers the selected model per profile and restores it across restarts. The memory lives in a `workshop-state.json` file in the server's state directory. A missing, unreadable, or corrupt memory file never blocks startup; the application starts with no memory and selects the first catalog model.

## The model cache

You can trigger a download of a model blob into the gateway's cache and watch cumulative progress until the blob is ready or the download fails. When the requested blob is already cached, you get an immediate ready answer instead of a download. The cache feature is meaningful only in the standard local deployment, where the Workshop and the gateway run on the same machine and share the filesystem.

Before the application receives its first state from the server, you see an empty workbench: no profiles, no active profile, no selected model, and chat gated off. Every server push refreshes the Model menu and chat gating, even when nothing changed, so the display never goes stale.

You now have a model selected and chat ready. The next chapter teaches the chat surface itself.

