# Editing Configuration Safely

This chapter teaches you the safe-edit surface: how the gateway stages edits in shadow files, how you preview and apply them, and how you recover when an edit is wrong. Editing through this surface means a bad config can never take down a running gateway.

## Shadow files

Pending admin edits are staged in one shadow file, `gateway.toml.next`, beside the real config. No save touches a real file until promotion.

Stage a full config edit with PUT /admin/config. The request takes the same JSON shape that GET /admin/config returns. Secrets left as the redacted marker `***` are restored from the current values, and a marker with no existing value fails validation. The merged result is validated like a real load before any shadow is written. The reply names the shadow file that was written.

The profile selection is not a config key. A document that sets `active_profile` is refused with a validation error pointing you at POST /admin/switch-profile, which the previous chapter covers.

## Preview before you apply

Preview the merged pending configuration with secrets still redacted:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-pending
````

The pending envelope also reports the persisted profile selection under `profile.active_profile`, read from the real `gateway.state.toml`: `null` when no profile is selected, otherwise the stored name even when the running profile differs or the config no longer defines it.

Poll a cheap dirty report of pending shadow files and changed sections:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-dirty
````

## Apply

Applying a pending edit is an explicit promote step:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-apply
````

The real file is replaced atomically. On platforms where rename cannot overwrite, a backup-and-restore fallback preserves the old file. The reply reports `applied`, `reloaded`, and `restart_required`.

What an apply does depends on which sections changed. The remote-facing sections - `[[model]]`, `[[endpoint]]`, `[[dominion]]`, and `[tools]` - reload live: the gateway rebuilds the remote routing table from the applied config, keeps the running local models under it, and swaps the routing table in one write. Nothing drains, nothing stops, and no child process starts. The boot-owned sections - `[server]`, `[workshop]`, `[[profile]]`, `[[local_model]]`, `[[stt_model]]`, and `[stt]` - promote to disk but take effect at the next start, so the reply sets `restart_required: true`; an env shadow does the same. The gateway's local model set is fixed for the process lifetime, so an edit that adds, removes, or changes a local or speech model, or changes a profile's checklist, always needs a restart. One apply can do both: reload the remote catalog now and report a restart for the rest.

An apply that changes the config runs as a command on the gateway's command queue, the same queue that runs the boot load. The queue is one serialized pending deque with no fixed capacity: debounce decides what stays pending, and a single worker runs the surviving commands in order. The request waits for the command's outcome, so the call above still returns when the apply is done. While the command runs, `GET /admin/status` reports it as the active command named `apply-config`, and its one `applying-config` stage streams on the live progress stream; the config UI's Apply overlay follows it and shows a Cancel button. `POST /admin/queue/cancel` stops it; the request then answers 503 with error code `apply_cancelled`. An apply requested while the boot load is still running waits behind it, so a reload never races the boot's publication of local models. An apply that touches only the env file, or only boot-owned sections, needs no reload and runs inline without a command.

Promotion happens at the end. The shadow is read into memory when the apply is requested, the new routing table is built, and only then are the captured bytes written to the real file and the shadow removed. A cancelled or failed apply therefore promotes nothing: the shadow stays on disk, the pending count stays where it was, and the next Apply runs the whole thing again. A save that lands while an apply is in flight is kept as the next pending change, never silently lost and never half-applied.

## Revert

Discard every staged edit without touching the real files:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-revert
````

The reply names the deleted shadow files. Deleting the shadows is the whole revert. A revert never touches the profile selection, because the selection is never staged.

## The .env file

Read and stage the gateway's global `.env` file over the same surface. GET /admin/env returns the file with plaintext values and shows which config fields reference each variable. PUT /admin/env stages a `.env.next` shadow that takes effect after restart. Variable names must use letters, digits, and underscores, and must not start with a digit. Values must round-trip through the dotenv parser.

## Failure behavior

You are protected from half-applied state. Saves, revert, profile selection, and the apply's snapshot and commit steps serialize on one lock, and applies serialize with the boot load on the command queue. An invalid pending config is never promoted; the request fails before any command exists. A failed or cancelled apply leaves every shadow on disk for correction, retry, or revert. A revert issued during an apply cancels the apply first, so the apply's commit never writes over files you just reverted.

