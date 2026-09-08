# shared-loopback

Shared loopback trust-boundary checks for the Gateway and Workshop products.

- This crate solely owns peer and host-authority checks for loopback-bound product surfaces. Consumers call these checks instead of reimplementing them.
- Fail closed: a request missing `ConnectInfo<SocketAddr>` is refused as non-loopback, never admitted on a wiring fault; the server must start with `into_make_service_with_connect_info::<SocketAddr>()`. A request naming no authority (no URI authority, no `Host` header) is refused by the host check the same way.
- The host check enforces only while the bound address is loopback; a non-loopback bind passes every authority, so a LAN server keeps serving its network.
- `gateway_loopback_origin_allowed` and `workshop_same_origin_authority_allowed` are separately named, fail-closed predicates with distinct policies; never merge or share their policy semantics.
- Keep this boundary lean so headless Gateway builds do not acquire UI or embedded-asset dependencies.
