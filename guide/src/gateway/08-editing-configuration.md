# Editing Configuration Safely

This chapter teaches you the safe-edit surface: how the gateway stages edits in shadow files, how you preview and apply them, and how you recover when an edit is wrong. Editing through this surface means a bad config can never take down a running gateway.

## Shadow files

Pending admin edits are staged in shadow files: `gateway.toml.next` and `gateway.state.toml.next`. No save touches a real file until promotion.

Stage a full config edit with PUT /admin/config. The request takes the same JSON shape that GET /admin/config returns. Secrets left as the redacted marker `***` are restored from the current values, and a marker with no existing value fails validation. The merged result is validated like a real load before any shadow is written.

## Preview before you apply

Preview the merged pending configuration with secrets still redacted:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-pending
````

Poll a cheap dirty report of pending shadow files and changed sections, including `active_profile`:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-dirty
````

## Apply

Applying a pending edit is an explicit promote step:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-apply
````

The real file is replaced atomically. On platforms where rename cannot overwrite, a backup-and-restore fallback preserves the old file. The reply carries `applied`, `reloaded`, and `restart_required`. The reply tells you when an edit needs a process restart to take effect: an env shadow or a change to `[server]` or `[workshop]` requires a restart. The apply's reload stages stream on the live progress stream; the apply response carries only the outcome.

An apply that changes the config or the state runs as a command on the gateway's command queue, the same queue that runs profile switches and boot provisioning. The request waits for the command's outcome, so the call above still returns when the apply is done. While the command runs, `GET /admin/status` reports it as the active command named `apply-config`, and the config UI's Apply overlay follows its stages and carries a Cancel button. `POST /admin/queue/cancel` stops it; the request then answers 503 with error code `apply_cancelled`. An apply supersedes any profile switch in flight, including the boot load, because the applied configuration is the one you want running; a profile switch requested during an apply waits behind it. An apply that touches only the env file, or only a process-owned section, needs no reload and runs inline without a command.

Promotion happens at the end. The shadow files are read into memory when the apply is requested, the new configuration is downloaded and started, and only then are the captured bytes written to the real files and the shadows removed. A cancelled or failed apply therefore promotes nothing: every shadow stays on disk, the pending count stays where it was, and the next Apply runs the whole thing again. A save that lands while an apply is in flight is kept as the next pending change, never silently lost and never half-applied.

## Revert

Discard every staged edit without touching the real files:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-revert
````

The reply names the deleted shadow files. Deleting the shadows is the whole revert.

## Profiles and shadows

You can switch the active profile immediately without consuming an unapplied state shadow staged by the config UI. Loading prefers shadow files over real files, while command-line and environment profile selections still outrank pending state.

## The .env file

Read and stage the gateway's global `.env` file over the same surface. GET /admin/env returns the file with plaintext values and shows which config fields reference each variable. PUT /admin/env stages a `.env.next` shadow that takes effect after restart. Variable names must use letters, digits, and underscores, and must not start with a digit. Values must round-trip through the dotenv parser.

## Failure behavior

You are protected from half-applied state. Saves, revert, and the apply's snapshot and commit steps serialize on one lock, and applies serialize with profile switches on the command queue. An invalid pending config is never promoted; the request fails before any command exists. A failed or cancelled apply leaves every shadow on disk for correction, retry, or revert. A revert issued during an apply cancels the apply first, so the apply's commit never writes over files you just reverted. A failed state-shadow write rolls the config shadow back to its previous contents.

