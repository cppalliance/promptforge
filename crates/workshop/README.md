# crates/workshop/

`crates/workshop/` is the workshop family's private container - nothing outside may depend in, and inside it dependencies flow one way: shell -> features -> services -> vocabulary.

## workshop

The desktop app (at `shell/`): hosts the workshop server in-process and opens the workshop window. It is the shipped artifact, and it reaches the server only through workshop-server-api. Depends on workshop-server-api and gateway-api-discovery; Tauri is the load-bearing third-party stack.

## workshop-server

The workshop HTTP server: serves the workshop API to the desktop shell, loopback-only, with the embedded SPA. The shell hosts it in-process, and it composes every subsystem through the registry. It also holds the sessions subsystem itself: the `/ws` workbench socket, the `/agents/ws` agent-session socket, and the `/v1/models` catalog relay, with agent sessions run in the harness through `harness-api` (the shell constructs the `Harness` at boot, registers it, and pushes the gateway binding, chat catalog, and host snapshot into it as data). Depends on all eight sibling subsystems plus harness-api, promptforge-api-types, shared-loopback, and gateway-api-discovery; build-ui is its build dependency.

## workshop-server-api

The shell's view of the server: re-exports only, so server internals never resolve in the shell. The shell depends on it and never on workshop-server. Depends on workshop-server.

## workshop-gateway

The gateway client: bearer-auth HTTP, endpoint binding and discovery, heartbeat, the progress subscriber (which decodes the gateway's `Progress` snapshots and drives the status bar's busy frames), and the run event log. The server's subsystems reach the gateway through it. Depends on workshop-protocol, workshop-registry, workshop-support, promptforge-api-types, gateway-api-types, and gateway-api-discovery.

## workshop-menu

The server-owned Model menu workbench: the snapshot, broadcast bus, chat model catalog, and per-profile model memory. The server mounts it as the menu subsystem. Depends on workshop-protocol, workshop-registry, and workshop-support.

## workshop-protocol

The wire protocol: every JSON frame over the workshop sockets, typed in one place, zero I/O. Every subsystem and the SPA share it as the frame contract. Depends on promptforge-api-types.

## workshop-registry

The sealed proxy slots subsystems self-register into, so the composition root never names them. The server builds its subsystem set through it. Depends on workshop-protocol.

## workshop-status

The status-bar broadcast bus. The server mounts it as the status subsystem. Depends on workshop-protocol, workshop-registry, and workshop-support.

## workshop-support

The support vocabulary: atomic writes, reconnect backoff, route deadlines, `workshop.toml`, and the retained broadcast bus. Every subsystem builds on it. No workspace dependencies.

## workshop-user-state

The account-scoped UI state bucket persisted as one JSON file in the state directory. The server mounts it, and the SPA's persisted account state lands here. Depends on workshop-protocol, workshop-registry, and workshop-support.

## workshop-workspace

The jailed filesystem behind `/workspace/*`: trees, reads, and writes confined to granted roots. The server mounts it as the workspace subsystem. Depends on workshop-protocol, workshop-registry, and workshop-support.
