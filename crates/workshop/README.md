# crates/workshop/

`crates/workshop/` is the workshop family's private container - nothing outside may depend in, and inside it dependencies flow one way: shell -> features -> services -> vocabulary.

## workshop

The desktop app (at `shell/`): hosts the workshop server in-process and opens the workshop window. It is the shipped artifact, and it reaches the server only through workshop-server-api. Depends on workshop-server-api and gateway-api-discovery; Tauri is the load-bearing third-party stack.

## workshop-server

The workshop HTTP server: serves the workshop API to the desktop shell, loopback-only, with the embedded SPA. The shell hosts it in-process, and it composes every subsystem through the registry. Depends on all nine sibling subsystems plus shared-loopback, shared-progress, and gateway-api-discovery; build-ui is its build dependency.

## workshop-server-api

The shell's view of the server: re-exports only, so server internals never resolve in the shell. The shell depends on it and never on workshop-server. Depends on workshop-server.

## workshop-gateway

The gateway client: bearer-auth HTTP, endpoint binding and discovery, heartbeat, the progress subscriber, and the run event log. The server's subsystems reach the gateway through it. Depends on workshop-protocol, workshop-registry, workshop-support, promptforge-api-types, shared-progress, and gateway-api-discovery.

## workshop-menu

The server-owned Model menu workbench: the snapshot, broadcast bus, chat model catalog, and per-profile model memory. The server mounts it as the menu subsystem. Depends on workshop-protocol, workshop-registry, and workshop-support.

## workshop-protocol

The wire protocol: every JSON frame over the workshop sockets, typed in one place, zero I/O. Every subsystem and the SPA share it as the frame contract. Depends on promptforge-api-types.

## workshop-registry

The sealed proxy slots subsystems self-register into, so the composition root never names them. The server builds its subsystem set through it. Depends on workshop-protocol.

## workshop-sessions

The sockets: the `/ws` workbench, `/agents/ws` agent sessions with supervision and input waits, and the `/v1/models` catalog relay. The server mounts it as the session subsystem, and the SPA's sockets are its client half. Depends on workshop-gateway, workshop-menu, workshop-protocol, workshop-registry, workshop-support, promptforge-api-runtime, promptforge-api-types, and shared-vfs.

## workshop-status

The status-bar broadcast bus and the progress renderer driven by the process progress hub. The server mounts it as the status subsystem. Depends on workshop-protocol, workshop-registry, workshop-support, and shared-progress.

## workshop-support

The support vocabulary: atomic writes, reconnect backoff, route deadlines, `workshop.toml`, and the retained broadcast bus. Every subsystem builds on it. No workspace dependencies.

## workshop-user-state

The account-scoped UI state bucket persisted as one JSON file in the state directory. The server mounts it, and the SPA's persisted account state lands here. Depends on workshop-protocol, workshop-registry, and workshop-support.

## workshop-workspace

The jailed filesystem behind `/workspace/*`: trees, reads, and writes confined to granted roots. The server mounts it as the workspace subsystem. Depends on workshop-protocol, workshop-registry, and workshop-support.
