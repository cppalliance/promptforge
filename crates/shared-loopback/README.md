# shared-loopback

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The shared loopback wall for the PromptForge gateway's config surface: two middlewares, one per signal. `require_loopback` refuses any request whose peer address is not loopback; `require_loopback_host` refuses any request whose authority is not the bound loopback socket, closing DNS rebinding. The gateway depends on this crate unconditionally and layers the peer check over its admin config endpoints (config read/write, env, pending state, apply/revert, orphans, system, model-info, the HF proxy, profile create/delete, reveal, shutdown, the `/auth` handoff) and the host check over its whole surface when bound to loopback, so the wall exists in every build - including headless builds that never compile the optional `gateway-config-ui` crate, which re-exports the peer check for its SPA asset routes. The crate is deliberately tiny: axum is its only dependency.

## Public surface

- `require_loopback` - an axum middleware (applied through `axum::middleware::from_fn`) that answers `403 Forbidden` to any peer that is not loopback, before bearer auth ever runs. It reads the peer address from the `ConnectInfo<SocketAddr>` request extension (present when the server is started with `into_make_service_with_connect_info::<SocketAddr>()`) and fails closed: a request with no peer address is refused as non-loopback.
- `require_loopback_host` - an axum middleware (applied through `axum::middleware::from_fn_with_state` with the bound `SocketAddr` as state) that answers `403 Forbidden` to any request whose authority is not the bound loopback socket. While the bind is loopback, only the socket's literal `ip:port` form and `localhost:port` are admitted (plus the port-elided bare forms on a port-80 bind, since http clients omit the default port); a request naming no authority fails closed. A non-loopback bind admits every authority, since a LAN server has no loopback allowlist to enforce. No route is exempt, `/health` included: the connection-file health probe sends the bound address as `Host`.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
