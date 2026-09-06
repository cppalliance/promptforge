# shared-loopback

Shared loopback trust-boundary checks for the Gateway and Workshop products.

- `require_loopback` is the only peer check for Gateway admin config and config-ui SPA routes, and `require_loopback_host` is the only host-authority check for the Gateway's loopback-bound surface; never reimplement either check in Gateway or config-ui.
- Fail closed: a request missing `ConnectInfo<SocketAddr>` is refused as non-loopback, never admitted on a wiring fault; the server must start with `into_make_service_with_connect_info::<SocketAddr>()`. A request naming no authority (no URI authority, no `Host` header) is refused by the host check the same way.
- The host check enforces only while the bound address is loopback; a non-loopback bind passes every authority, so a LAN server keeps serving its network.
- `gateway_loopback_origin_allowed` and `workshop_same_origin_authority_allowed` are separately named, fail-closed predicates with distinct policies; never merge or share their policy semantics.
- Stay tiny: axum is the only dependency so headless gateway builds can take the wall without pulling config-ui or embedded-asset machinery.
