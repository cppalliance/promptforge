# promptforge-web-search

This crate owns the concrete `web_search` tool provider through the Gateway endpoint.

- Tool vocabulary comes from `promptforge-tools`. This provider never depends on Core or a Gateway product crate.
- The bearer credential, endpoint validation, request deadline, argument bounds, and response decoding stay in this provider.
- Errors preserve their sources: wrap the underlying cause with `ToolError::with_source` instead of flattening it into the message.
- Every request is bounded: a fixed deadline on the HTTP client and each outbound call, capped argument sizes, and response bodies that reject a cap overflow rather than truncating.
- Diagnostics are secret-free: the bearer token never appears in `Debug`, `Display`, or an error message, and a rejected endpoint is described without echoing a URL that could embed credentials.
