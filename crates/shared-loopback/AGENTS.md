# shared-loopback

The single shared loopback wall for the gateway: two middlewares, one per signal. `require_loopback` refuses non-loopback peers before auth; `require_loopback_host` refuses authorities that are not the bound loopback socket (DNS-rebinding defense).

- This crate is the only loopback check for admin config and config-ui SPA routes, and the only host-authority check for the gateway's loopback-bound surface; never reimplement either check in gateway or config-ui - all three must call through here so the wall cannot drift.
- Fail closed: a request missing `ConnectInfo<SocketAddr>` is refused as non-loopback, never admitted on a wiring fault; the server must start with `into_make_service_with_connect_info::<SocketAddr>()`. A request naming no authority (no URI authority, no `Host` header) is refused by the host check the same way.
- The host check enforces only while the bound address is loopback; a non-loopback bind passes every authority, so a LAN server keeps serving its network.
- Stay tiny: axum is the only dependency so headless gateway builds can take the wall without pulling config-ui or embedded-asset machinery.
