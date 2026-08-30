# promptforge-gateway-loopback

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The shared loopback wall for the PromptForge gateway's config surface: one middleware, `require_loopback`, refusing any request whose peer address is not loopback. The gateway depends on this crate unconditionally and layers the middleware over its admin config endpoints (config read/write, env, pending state, apply/revert, orphans, system, model-info, the HF proxy, profile create/delete, reveal), so the wall exists in every build - including headless builds that never compile the optional `promptforge-gateway-config-ui` crate, which re-exports the same function for its SPA asset routes. The crate is deliberately tiny: axum is its only dependency.

## Public surface

- `require_loopback` - an axum middleware (applied through `axum::middleware::from_fn`) that answers `403 Forbidden` to any peer that is not loopback, before bearer auth ever runs. It reads the peer address from the `ConnectInfo<SocketAddr>` request extension (present when the server is started with `into_make_service_with_connect_info::<SocketAddr>()`) and fails closed: a request with no peer address is refused as non-loopback.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
