# workshop-server

This crate owns the Workshop HTTP and WebSocket server and its host-embeddable spawn surface.

- Construction errors return rich causes to the host. Request and session failures become wire errors, status reports, or logged degradation instead of panics. Gateway outages are request-time failures.
- Do not install global tracing state or retain process-global initialization that ignores host arguments. Bind and initialization failures return through the spawn handshake.
- The Workshop listener binds only to loopback.
- The Realtime transcription relay authenticates upstream, validates browser origin, and forwards opaque speech payloads without owning speech state.
- One task owns each socket, its protocol policy, and its cleanup. Agent sessions may survive socket disconnect; other per-request relay work does not gain a session registry.
- Every pushed message type is durable or ephemeral. Durable delivery supports replay and duplicate tolerance; ephemeral delivery may coalesce or drop under lag and restores its latest complete snapshot after reconnect.
- Work held for a disconnected client cancels through its ownership guard.
- Application state is composed at boot: each subsystem registers its handles into `workshop-registry`, and the shell asserts the composition at startup. Runtime reads of absent optional contributions degrade to no-ops. Do not pass one subsystem's handles into another subsystem's constructor, and do not reintroduce per-request panics on missing registrations.
- Asset construction failures return to the host. API-path misses return 404 instead of the SPA index.
- Held sockets and uncooperative clients must not make server shutdown unbounded.
