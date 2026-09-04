# The Configuration UI

The gateway serves a browser UI for configuration: you reach it over HTTP, sign in with your API key, and edit every part of the configuration through its views. The UI rides the safe-edit surface from the previous chapter, so everything you do there moves through pending shadows and Apply.

## Reach the UI

The gateway serves the configuration UI at /config on its own port; there is no second listener. The UI is an optional feature you compile in with `config-ui`, which is on by default. GET /config redirects permanently to /config/. Four asset endpoints live under /config: the index page at /, the bundled script at /app.js, the stylesheet at /app.css, and the program icon at /icons/promptforge-icon-1.png.

The UI pages need no bearer token, but every asset route answers 403 Forbidden to any peer that is not loopback. The UI is reachable only from the gateway machine itself, and the check fails closed.

## Sign in

On first load without a stored key, you see a "PromptForge Gateway" sign-in card with a labeled API key password field and a Connect button. A wrong key shows "Invalid API key". An unreachable gateway shows "Gateway unreachable". The verified key is stored for the browser session. Any later 401 from the gateway clears the stored key and returns you to the key prompt.

## Get oriented

You navigate six top-level views from the tab bar: Settings, Discover, Local, Remote, Profiles, and Secrets. Every view has a bookmarkable hash URL, including a specific model's detail page and a specific Settings section. An unrecognized hash is rewritten to #/local.

A connection dot in the tab bar shows whether the gateway is reachable. The tab bar shows the running UI's version as a muted label, or vdev for a development build. Notifications appear as toasts that dismiss themselves after four seconds. Destructive actions require confirmation in a modal dialog that names the target; focus lands on Cancel as the safe default, and Escape or a backdrop click cancels. Every dropdown works entirely from the keyboard, including typing the first letters of an option to jump to it. The UI is always dark, the reduced-motion system preference disables essentially all animation, and byte sizes appear in human-readable units.

## The three states of an edit

Edits move through three states: unsaved edits held in the browser, saved pending shadows on the gateway, and the applied running configuration. When pending changes exist, the tab bar shows an Apply button labeled with the pending file count beside a Revert All button. When a previous session left unapplied changes, a banner offers Review, Apply, and Revert All.

Pressing Apply opens a progress overlay that follows the gateway's live progress stream stage by stage until the apply finishes or fails. A failed stage holds on the error message for a moment before the overlay closes. When an applied configuration requires a restart, a banner reads "Restart the gateway to apply these changes." and clears itself once the gateway comes back on a new config generation.

Open the Review dialog to list every pending configuration change as a table of path, running value, and pending value. Secret values are never displayed.

## Profiles

The tab bar shows the active profile name. A menu lists every profile with the pending choice checked. Choosing another profile stages `active_profile` as a pending change that takes effect on Apply. A failed staging surfaces an error toast and leaves the current selection unchanged.

In the Profiles view you edit each profile as an ordered subset of the global model catalog through Available and Chosen shuttle listboxes. The listboxes support multi-select, roving focus, typeahead, selection counts, and per-pane search. The profile saves in global catalog order, not click order. A new profile starts Empty or as a Copy of an existing profile. You cannot delete the profile currently staged as active. The Set Active button stages the active profile; once staged it reads "Selected for Apply", and the switch lands on Apply.

The Profiles view shows an Estimated VRAM summary that sums declared model weights. Per-dominion budget rows warn at 80 percent and error when over. KV cache grows with context length, so 20 percent headroom is recommended.

## Discover

The Discover view searches Hugging Face. The search box accepts keywords, a `user/repo` form, or a pasted hub URL, and keystrokes collapse into one search after a 300 ms debounce. The GGUF filter chip is locked on because the gateway serves GGUF inference only. Chat is the default workload filter; the filters cover Chat, Embedding, Reranker, STT, Image, and TTS. Result rows show the publisher avatar, a parameter-count pill, compact download and like counts, and a relative updated time. Sorts are Most downloads, Trending, and Newest.

A model's GGUF files are grouped into named quantizations with exact summed byte sizes and the LFS SHA-256 for single-file quants, listed smallest first. Each quant shows a fit badge computed against the gateway's system snapshot: Fits GPU, Partial offload, CPU only, or Too large. One Recommended star marks the largest quant that fully fits free VRAM. A multi-part GGUF cannot be downloaded as one model; the button is disabled with an explaining tooltip. You can read model cards in the view, rendered as sanitized HTML so embedded scripts and event handlers cannot execute.

A Download click stages a pending model entry carrying the hub resolve URL, the LFS digest, and the listing size as `vram_gb`; Apply owns the actual transfer. Staging a discovered model also adds it to the active profile's checklist, so Apply provisions and serves it. The staged entry prefills a mapped built-in chat template when the server-side catalog matches the repo. An STT-filtered download stages a first-class `stt_model` entry with the interim role. Without a configured HF token you see a banner linking to the Secrets view instead of search results.

## Local and Remote

The Local and Remote views show the gateway's own catalog subsets: Local lists your local and speech-to-text entries, and Remote lists your remote entries. STT entries carry a Mic badge so you can pick them out at a glance. Filter chips narrow the list to All, Chat, or STT, a search box filters the rows after a short debounce, and a sort dropdown orders the list by Name, Size, or Kind.

Each model row shows a running-status dot, a kind badge, and capability pills. A quant badge read from the GGUF filename names the quantization, such as Q4_K_M. A model that exists only as a draft carries an "unsaved" badge until you save it.

## Secrets

The Secrets view manages the one global `.env` file. Variables appear as masked password rows with per-row reveal and delete. Save stages a pending shadow that takes effect only after Apply plus a gateway restart. New variable names must use letters, digits, and underscores, and must not start with a digit. Each variable shows "used by" annotations naming the configuration entries that reference it. A dedicated Hugging Face card configures `HF_TOKEN`, and its Test Connection probes the token the running gateway holds and reports Not set, Valid, Invalid, or Connection failed.

## Settings

The Settings view has seven sections: System, Gateway, Workshop, Dominions, Endpoints, Tools, and About. You land on System by default.

The System panel shows live metric tiles: CPU, RAM, VRAM with the GPU name, and disk usage with the cache path. The tiles refresh every 5 seconds, and a failed refresh keeps the last snapshot. Metric bars recolor by load: warning in the 70 to 89 percent band and danger at 90 percent or more.

The Gateway card edits the bind address and the API key. A note says the boot configuration cannot hot-reload, and changing the API key warns that the new key will be required after restart. The typed key leaves the DOM once saved. Stored secrets render as a masked readout with a Change button; leaving the input empty keeps the existing key, and an Eye toggle reveals and re-hides the secret.

The Dominions and Endpoints cards show used-by chips that count dependents, and a delete confirmation names them. A local-kind dominion reveals the `vram_gb` budget field, and switching the kind to remote hides it. An endpoint binds to a dominion from a dropdown offering only remote-kind dominions plus None. The endpoint protocol dropdown is locked to `openai`, and the endpoint API key stays redacted through saves until Change reveals the input.

The Tools section configures web search with the provider locked to Brave and the defaults documented on the card. The Storage card edits the cache directory beside live cache-drive usage, with a warning that changing the directory does not move existing files.

The About panel shows the medallion, the baked version or "dev", and the Boost Software License link. The Config UI card reports the UI as compiled in by the `config-ui` feature, served on the gateway's own port, loopback only, with the URL derived from the bind. The Workshop card edits the `[workshop]` section's one live content, the STT capture tuning - the gateway hosts no workshop listener, so the section's old `bind` and `open_browser` settings are inert and stay out of the editor. Adding the tuning seeds window_seconds 15, interval_ms 500, and an empty vocabulary.

## Editing a model

You edit a local model through sections for GPU, generation, source, and capabilities in the model detail view. An unconfigured optional section offers an Add button. The chat template control offers Auto, a built-in template family, or a custom .jinja path, with a read-only summary naming the effective source, the detected family, and the reason.

The model name edits inline in the detail header, and the header shows the model's status: Unsaved, Running, or Stopped. Each edited field carries a dirty dot and a per-field reset. Each saved-but-unapplied field carries a pending chip whose tooltip shows the running value. Deleting a model confirms a dialog naming the model and every affected profile, and the save removes every dangling profile reference in the same payload. A downloaded model shows its cached size and path with a Delete file action. A path source gets a reveal-in-folder button; URL sources get none. Capability pills show images and thinking mode, and the images pill is implied and locked when a multimodal projector is configured. The `gpu_layers` slider readout carries the GGUF layer total, and typing "Max" maps to the maximum.

The controls follow the shape of the value. Numeric settings pair a slider with a typed readout, typed values clamp to the allowed range, wide-range settings such as the context window use a logarithmic scale, and some sliders offer a rightmost "Max" detent. List-valued settings such as a model's endpoint list are edited as removable chips. Fields with a fixed choice set accept only the listed values. Boolean settings use an on/off switch. A setting can be disabled until a sibling field holds a required value, or hidden until a predicate passes, so you only see applicable controls.

Retyping a field's original value clears its unsaved edit, and you can reset one field or a whole entry. A new model entry starts as an unsaved draft, and name collisions get auto-suffixed. Every settings save carries the complete single-file configuration, so one section's save never erases another staged section.

An orphan section lists unconfigured files on disk with Adopt and Delete actions per file; Delete is disabled when the file has no verified digest. The UI shows whether a model's source file is already downloaded. On gateways built without local-model features, missing orphan and chat-template endpoints degrade to empty lists instead of breaking the UI. You can restore the recommended speech-to-text model pair, digest-pinned, over the existing STT catalog entries from the UI.

## Panel mode

The configuration UI runs in two modes. Standalone mode runs in a browser tab. Panel mode embeds the UI inside the Workshop with `?mode=panel`. In panel mode your API key never enters the frame; every gateway call rides a postMessage bridge to the Workshop, and the panel only talks to a loopback workshop origin. Bridged calls fail after a 30 second reply deadline rather than hanging. Apply and Revert actions are announced to the workshop's status bar, and the workshop pushes its theme and an initial route into the embedded panel once the bridge is up.

