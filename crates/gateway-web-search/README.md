# gateway-web-search

The gateway-side web-search service: request validation for
`POST /v1/tools/web_search` - the Brave Search provider client,
result post-processing (sanitize, tracking strip, domain filters, host
diversity caps), and `WebSearchState`.

The gateway owns the route, the bearer-auth check, and the profile-switch reload; it builds a `WebSearchState` from the active profile's
`[tools.web_search]` section and calls `WebSearchState::search`. The
provider credential stays inside `gateway_config::Secret`, which
redacts in `Debug` and `Display`, and leaves this crate only at the provider
call site.
