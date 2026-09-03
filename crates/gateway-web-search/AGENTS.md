# gateway-web-search

This crate owns the gateway-side web-search service: the Brave Search
provider client, request validation, result post-processing, and
`WebSearchState`.

## Rules

- Search provider service only: no HTTP routing, no bearer-auth policy, no
  profile switching. The gateway mounts the route, checks the credential,
  and swaps the state on profile switch.
- Credentials never appear in `Debug` or `Display` output: the provider key
  stays inside `gateway_config::Secret` and is exposed only at
  the provider call site.
- The crate never names gateway concepts (`GatewayError`, `AppState`,
  `check_auth`); failures return its own `WebSearchError`, which wraps
  `ProtocolError` from `gateway-protocol`.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
