# gateway-routing

Routing vocabulary for the PromptForge inference gateway: the `Model` and
`Endpoint` routing-table entries and the per-dominion admission control
(`DominionQueue`, `ClientId`, `Permit`, `AdmitError`, `dominion_queues`).

This crate is the shared data plane between the gateway (which owns the
`Routing` table and the HTTP routes) and `gateway-local` (which
builds `Model` entries for managed `llama-server` children). It resolves no
model names, serves no HTTP, and constructs no upstreams.

One feature flag exists:

- `test-helpers` - exposes the `DominionQueue` observation seams
  (`waiter_count`, `distinct_clients`) to downstream crates' test suites.
