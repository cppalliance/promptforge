# shared-loopback

The single shared loopback wall for the gateway config surface: one `require_loopback` middleware that refuses non-loopback peers before auth.

- This crate is the only loopback check for admin config and config-ui SPA routes; never reimplement the peer check in gateway or config-ui - both must call through here so the wall cannot drift.
- Fail closed: a request missing `ConnectInfo<SocketAddr>` is refused as non-loopback, never admitted on a wiring fault; the server must start with `into_make_service_with_connect_info::<SocketAddr>()`.
- Stay tiny: axum is the only dependency so headless gateway builds can take the wall without pulling config-ui or embedded-asset machinery.
