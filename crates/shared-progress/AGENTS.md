# shared-progress

This crate owns the progress vocabulary and nothing else: operation-scoped weighted progress trees, the process-wide `ProgressHub` broker, reporting handles with coalesced event emission, the serde-gated wire events, remote import, and pull-based snapshots.

- Zero promptforge dependencies. This crate sits at the bottom of the workspace graph: every other crate may depend on it, and it depends on none of them.
- Runtime-agnostic. The crate never spawns tasks and never blocks. It exposes a `tokio::sync::broadcast` of events plus pull-based snapshots; hosts spawn their own forwarding and renderer tasks.
- Producers report through `ProgressHandle` and never see events or serde; renderers subscribe to the hub or pull snapshots. Never invent a parallel progress channel: no ad-hoc callbacks, stage strings, or direct status-bus calls for fractional progress.
- Intermediate events are lossy (coalesced at the source, droppable under receiver lag); terminal `Finished` events are never coalesced. Consumers detect completion only from `Finished`, never from a fraction reaching 1.0.
- Weights are proportional to expected time, not bytes or unit counts: a leaf's byte total is how it computes its own fraction, never its weight.
- The `serde` feature only gates `Serialize`/`Deserialize` derives on the wire types. Features stay additive: enabling one may add capability, never remove or rename it.
