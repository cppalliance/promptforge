# workshop-workspace

The PromptForge Workshop's workspace subsystem: confined filesystem access behind `/workspace/*` - directory trees, file reads, and file writes jailed to roots the user explicitly granted - plus the workspace file that keeps those grants, the window geometry, and the workspace-scoped UI state (dock layout, expanded folders, closed editors) between sessions.

## Confinement

A dropped folder becomes a granted root; a dropped file grants its parent directory. Every request path is checked lexically (no `..`, and on Windows no NTFS alternate data stream names), then canonicalized and prefix-matched against the canonical grants before any filesystem operation, so traversal, symlink escapes, and UNC aliases cannot reach outside a grant. The in-memory grant set is the confinement source of truth. The workspace file below mirrors it and is never consulted on a request path.

## The workspace file

A workspace is one user-visible file, `Name.pfwork`: an embedded Turso database the user opens, saves as, and duplicates from the SPA File menu. Until the first Save As the workspace is ephemeral - grants live in memory only, nothing persists, and the display name is `Untitled`. Once a file backs the workspace, every grant and revoke lands in it as it happens, so there is no dirty state to save or lose: the file is a live mirror, not a snapshot.

While a file backs the workspace, turso runs it in write-ahead-log mode, so a `Name.pfwork-wal` sidecar sits beside the file and holds recent writes. `Workspace::close_backing` stops the actor and closes the connection, which checkpoints the log into the main file and removes the sidecar; graceful shutdown runs it through the subsystem's registered task, so a normal quit leaves exactly one file. A crash skips the close and leaves the sidecar; turso replays it into the main file on the next open, so nothing is lost. Because turso keeps a process-wide registry keyed by path, the same file is never opened twice in one process: an open of the already-open path reloads the existing backing instead (see `Workspace::open_file`).

Save As and Duplicate both create exactly one file at the chosen path. No directory is created around it; follow-on projects grow plain-named sibling directories (`agents/`, `runs/`) beside the file lazily, only when there is something to put in them. Two `.pfwork` files in one folder would share those siblings, so the convention is one workspace per folder. It is a convention, not an enforced rule.

Save As and Duplicate differ by what travels. Save As writes the current grants and the saved window state into a new file and switches to it; siblings stay beside the original. Duplicate drains pending writes, checkpoints the write-ahead log into the main file so the copy is complete without a `-wal` sidecar, copies the file plus any existing sibling directories except the derived `index.db`, and switches to the copy. In v1 no siblings exist, so both are the same file operation.

### Schema v1

The `user_version` pragma is the migration counter and holds `1`.

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE grants (
    path     TEXT PRIMARY KEY,  -- canonical, verbatim-prefix-free
    position INTEGER NOT NULL,  -- insertion order
    added_at TEXT NOT NULL      -- RFC 3339
);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL         -- JSON text
);
```

`meta` carries `format` (always `promptforge-workspace`), `version` (`1`), `name` (the display name; absent means the file stem), and `created_at`. `grants.position` and `added_at` record insertion order from both producers: a `grant()` on the live workspace assigns one past the current maximum and the current time, and Save As writes the in-memory grants with the `position` and `added_at` they were loaded or granted with, so the new file carries the true history rather than a renumbering. Removal never renumbers. The tree still lists grants in canonical path order, so the columns record history and do not change display. The table names `agent_windows`, `run_presets`, `runs`, `run_events`, `agents`, and `documents` are reserved for follow-on projects and unused.

#### The `kv` table

`kv` holds one JSON text per key. `window` is typed and written by the shell's own route; the other three are the opaque workspace-scoped UI state bucket - stored verbatim, checked only for an allow-listed key, a 1 MiB cap on the JSON text, and that it parses - whose schemas the SPA owns. Keys are additive, so `user_version` stays `1` and an older file reads every missing key as `null`.

| Key | Shape | Holds |
|---|---|---|
| `window` | `{ width, height, x, y, maximized }` in logical pixels | The shell's geometry. |
| `layout` | `{ version, zones, overrides, layout }` | The dock layout envelope exactly as the SPA's layout persistence builds it (schema version 3 today). |
| `tree` | `{ "expanded": ["<absolute path>", ...] }` | The folders expanded in the Workshop tree. Paths are absolute, matching the grants table. |
| `closed_editors` | `{ "paths": ["<absolute path>", ...] }` | The closed-editor stack, most recent first, capped at 50. |
| `scroll` | reserved | Unused. |
| `agent_sessions` | reserved | Unused. |

Opening validates `user_version`, `meta.format`, and `meta.version` before reading anything else and writes nothing. A file that is not a database, or a database without the stamp, is refused as "not a promptforge workspace file" and left byte-identical; a stamp at another version is refused with the found-versus-supported versions. A refusal never wipes or partially loads a workspace.

All database I/O runs on one actor task that owns the one connection; handles are clone-cheap senders into a bounded channel, so channel order is disk order and the synchronous confinement code keeps its shape. Dropping every handle drains the queue and closes the connection.

### Endpoints

Registered through `workshop_registry::Registry` beside the tree and file routes. Every successful switch answers with the workspace as it now stands, `{ path, name, grants, window_state }`, with `path` and `window_state` `null` while ephemeral.

| Route | Body | Effect |
|---|---|---|
| `GET /workspace/file/current` | none | Reports the open file, name, grants (each with `exists`), and saved geometry. |
| `POST /workspace/file/open` | `{ path }` | Opens the file and replaces every grant with its contents. |
| `POST /workspace/file/save_as` | `{ path }` | Creates a new file from the current grants and window state and switches to it. |
| `POST /workspace/file/duplicate` | `{ path }` | Copies the current file and its siblings to `path` and switches to the copy. While ephemeral there is no file to copy, so it behaves as `save_as`: a new file from the current grants. |
| `PUT /workspace/file/window-state` | `{ width, height, x, y, maximized }` | Saves geometry; answers `{ saved: false }` and writes nothing while ephemeral. |
| `GET /workspace/file/state` | none | Answers every allow-listed `kv` state key (`layout`, `tree`, `closed_editors`) with its value, `null` where nothing has been put or while ephemeral. |
| `PUT /workspace/file/state/{key}` | any JSON value | Stores the body verbatim under `key`; answers `{ saved: false }` and writes nothing while ephemeral. |

Every write to the file, the state keys included, funnels through the one actor task, so there is a single writer per `.pfwork`. Save As copies grants and window state into the new file but not the state keys; the SPA writes them after the switch so that fact has one writer too.

Failures reach the wire through the crate's `WorkspaceError` envelope: a refused file is a client error carrying the required-versus-actual text with grants unchanged, a missing path is the ordinary not-found, a path already taken is a conflict. A state put with an unknown key or a body that is not JSON is `400`, a body over the cap is `413`; either changes nothing, ephemeral or not.

### The last-workspace pointer

`state_dir/last-workspace` is a plain-text file holding the path of the workspace that was open when the server last ran. It is written atomically after every successful open, save-as, or duplicate, and read once at boot: the server reopens the file before the listener serves, so readiness means the grants are already in place. A missing pointer is the ordinary first launch. An unreadable, empty, or non-UTF-8 pointer, a target that has vanished, or a file that is refused all log a warning and start the server ephemeral. Boot never blocks on it and never fails for it.

### Zone-two behavior

Persistence never decides whether an operation succeeds. A grant or revoke updates memory first and then persists through the backing when one is open; a persist that fails logs at warn and the operation still returns success. A pointer that cannot be written costs the next launch its reopen and nothing else. Restored grants log at info, and a grant whose path has vanished from disk still loads and lists as `exists: false` so the user can see it and revoke it. Opening a workspace file grants its stored directories to the confined file API, the same trust gesture as dropping a folder: deliberate user action, restored grants visible in the tree.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../../LICENSE).
