# workshop-user-state

The PromptForge Workshop's user-state subsystem: the account-scoped UI state bucket. It holds the values the SPA keeps per user rather than per workspace - editor toggles, zoom, the recent-files list, the command palette's history - persists them as one JSON file in the server's state directory, and serves them over `/user/state`.

## Tier

A feature crate. It may depend on `workshop-protocol`, `workshop-registry`, and `workshop-support`, and never on `workshop-workspace`, `workshop-server`, or any `gateway-*` or `promptforge-*` crate. The workspace-scoped bucket (dock layout, expanded tree folders, closed editors) is the `workshop-workspace` crate's business and travels with the `.pfwork` file; this crate knows nothing about workspaces.

## The state file

`state_dir/ui-state.json` is one JSON object, keyed by the allow-listed names, each value stored exactly as the SPA serialized it:

```json
{
  "editor_settings": { "wordWrap": true, "renderWhitespace": false, "renderControlCharacters": false, "columnSelection": false },
  "zoom": 1.1,
  "recent_files": ["<absolute path>", "..."],
  "commands_history": ["<command id>", "..."]
}
```

The server never interprets a value beyond checking that its key is allow-listed, that its JSON text is at most 1 MiB, and that it parses as JSON. The SPA owns every value's schema. A key this build does not know is kept and rewritten with the rest, so a newer build's value survives a round trip through an older one, but only the allow-listed keys are served.

The file is read once when the store is constructed and nothing is created until the first put. A missing file is the ordinary first launch. An unreadable file, one that does not parse, or one whose top level is not an object is corrupt state: logged once at warn and read as empty, to be replaced whole by the next put.

Every put updates the in-memory map under one mutex and rewrites the whole file through `workshop_support::write_atomic` on a blocking task, the same pattern `workshop-menu` uses for `workshop-state.json`. The lock is held across the write so two puts cannot land their rewrites out of order, and a crash leaves the old document or the new, never a truncation.

## Endpoints

Registered through `workshop_registry::Registry` and merged into the shell's API router under the default deadline tier.

| Route | Body | Effect |
|---|---|---|
| `GET /user/state` | none | Answers every allow-listed key with its stored value, `null` where nothing has been put. |
| `PUT /user/state/{key}` | any JSON value | Stores the body verbatim under `key` and answers `{ "saved": true }`. |

A put is judged key first, then body size, then shape, so the client is told about the cheapest mistake. Failures reach the wire through the crate's own `UserStateError` envelope: an unknown key or a body that is not JSON is `400` (`user_state_key`, `user_state_not_json`), a body over the cap is `413` (`user_state_too_large`), and a write that fails is `500` (`user_state_io`). The raw body still passes axum's default 2 MiB body limit before the handler sees it; that hard stop answers axum's own `413`.

## Failure posture

Zone two throughout. A refused put returns its error and writes nothing. A put whose write fails keeps the new value in memory - the map is the source of truth and the file is its mirror - logs at warn, and answers the server-error envelope; the SPA warns once and keeps working with its in-memory value. Boot never blocks on the state file and never fails for it.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../../LICENSE).
