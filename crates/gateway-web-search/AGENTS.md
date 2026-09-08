# gateway-web-search

This crate owns the Gateway-side web-search provider service.

- The Gateway owns HTTP routing, bearer authentication, and profile switching. This crate owns provider requests, validation, and result processing.
- Credentials never appear in `Debug` or `Display` output: the provider key stays inside `gateway_config::Secret` and is exposed only at the provider call site.
- Failures use crate-local errors and preserve protocol causes instead of depending on Gateway error types.
