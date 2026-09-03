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

