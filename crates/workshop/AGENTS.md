# Workshop crates

The Workshop family: the desktop app, the in-process server, the subsystems the server composes, and the SPA. Tiers are the ones `cargo test -p build-xtask` enforces. Dependencies flow one way: server, then features, then services, then vocabulary.

## Crates

- `desktop` (package `workshop`, outside the tiers): the Tauri desktop app. It spawns the server in-process through `workshop-server-api` and supervises the gateway sidecar.
- `server` (server): the Axum server and composition root. It holds the agent sessions in `agents` (their wire frames in `agents::wire`), the `/ws` workshop socket in `workshop_socket`, the socket helpers both share in `websocket`, and the asset, gateway-config, prompts, realtime, and health routes.
- `server-api` (outside the tiers): the desktop app's only view of the server.
- `gateway` (services): the gateway client (`client`), binding publication (`binding`), heartbeat, catalog and profile refresh (`refresh`), and progress feed (`progress`). Every production module is private; the crate root's re-exports are the one public path.
- `workspace` (features): the jailed filesystem with granted roots, plus the `.pfwork` workspace file.
- `menu` (services): Model menu state and the chat catalog.
- `user-state` (features): per-account UI state.
- `status` (services): the status bus.
- `protocol` (vocabulary): the `/ws` frame types and the agent input-wait frames. The agent-session frames live in the server, their only producer, so this crate depends on no product crate.
- `registry` (vocabulary): self-registration slots and the `Push` facade.
- `support` (vocabulary): shared primitives (including the retained broadcast bus and its `recv_or_pending` helper) and test fixtures.
- `ui` (not a Rust crate): the TypeScript SPA. The workspace tree panel lives in `parts/workspace/`, the File menu's open, save-as, and duplicate workspace commands in `parts/workspace-document/`, and the icons and panel dialog helper in `parts/shared/`.

## Runtime links

Subsystems never name each other. Three runtime links cross the registry's subsystem-named seams:

- The gateway drives the menu through `MenuPush`.
- Publishing a model catalog forces a menu reconcile through `Push::push_models_catalog`.
- Agent sessions read the workspace's granted roots through `WorkspaceRoots`.

## Desktop boundary

The desktop app depends only on `workshop-server-api`, never on `workshop-server`, so server internals do not resolve in the desktop app. `cargo test -p build-xtask` enforces this rule.
