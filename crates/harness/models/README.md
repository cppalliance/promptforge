# harness-models

The harness's model client: the HTTP transport that performs the engine's `Chat` effects against the gateway a client has bound, streaming deltas back to the session, and the `GET /v1/models` catalog fetch a host resolves model selections against. `GatewayClient::complete` sends the engine's request body, reads the SSE stream under the run's byte cap and timeout, folds it through the engine's shared reassembly, and returns one `Completion` with the client-side timing it measured. Private to the harness family; clients reach it through `harness-api`.
