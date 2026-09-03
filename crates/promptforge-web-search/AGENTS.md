# promptforge-web-search

This crate owns the concrete `web_search` tool provider: it proxies a search query through the gateway's `POST /v1/tools/web_search` endpoint with a shared bearer token, so the vendor search credential never leaves the server. That provider is the whole scope.

- Tool vocabulary (`Tool`, `ToolId`, `ToolOutput`, `ToolError`, and their kinds) comes from `promptforge-tools`. This crate never depends on `promptforge-core` or the gateway.
- Provider-only ownership: the bearer credential, gateway endpoint validation, request deadline, argument bounds, and response decoding live here and nowhere else. Do not move them into a shared crate and do not reacquire them from one.
- Errors preserve their sources: wrap the underlying cause with `ToolError::with_source` instead of flattening it into the message.
- Every request is bounded: a fixed deadline on the HTTP client and each outbound call, capped argument sizes, and response bodies that reject a cap overflow rather than truncating.
- Diagnostics are secret-free: the bearer token never appears in `Debug`, `Display`, or an error message, and a rejected endpoint is described without echoing a URL that could embed credentials.
