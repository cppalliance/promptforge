# promptforge-gateway-routing

This crate owns the routing vocabulary shared by the gateway and the local
inference subsystem: the `Model`/`Endpoint` table entries and the per-dominion
admission queues (`DominionQueue`, `ClientId`, `Permit`, `AdmitError`,
`dominion_queues`).

## Rules

- Shared routing vocabulary only: no HTTP handling, no upstream construction,
  no error envelopes, no local inference. The `Routing` table and
  `GatewayError` stay in the gateway; provisioning stays in
  `promptforge-gateway-local`.
- Every public item carries a `///` doc comment; behavior changes ship with
  tests in the same change.
