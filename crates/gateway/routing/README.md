# Gateway routing

This crate defines the routing table entries that map a model to its backends and the admission queues that limit concurrent work per resource pool. The gateway owns the routing table, and local inference adds its models to it through this crate. It resolves no model names and serves no HTTP.
