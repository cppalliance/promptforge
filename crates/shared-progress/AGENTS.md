# shared-progress

This crate owns runtime-agnostic progress vocabulary and delivery semantics.

- This crate stays at the bottom of the workspace graph with no PromptForge product dependencies.
- Hosts own forwarding tasks. This crate does not spawn work or block a runtime.
- Producers report through `ProgressHandle`; renderers consume hub events or snapshots. Producers never format output or create a parallel progress channel.
- Intermediate events are lossy (coalesced at the source, droppable under receiver lag); terminal `Finished` events are never coalesced. Consumers detect completion only from `Finished`, never from a fraction reaching 1.0.
- Weights are proportional to expected time, not bytes or unit counts: a leaf's byte total is how it computes its own fraction, never its weight.
- Serialization changes are additive so existing wire vocabulary remains valid.
