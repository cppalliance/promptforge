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

You are protected from half-applied state. Apply, revert, and saves serialize on one lock. An invalid pending config is never promoted. A failed apply leaves the rejected shadow on disk for correction or revert. A failed state-shadow write rolls the config shadow back to its previous contents.

